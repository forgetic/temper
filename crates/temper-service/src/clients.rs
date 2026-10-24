//! The clients child and secret-free root handoffs (client/README.md, 2 and 8; jig's
//! protocol/service.md, 3).
use crate::settings::Seeds;
use alloc::boxed::Box;
use jig_domain_core as core;
use jig_domain_people as people;
use jig_protocol_config::{self as config, Path, Problems, What};
use jig_protocol_engine_clients as kit;
use jig_protocol_engine_clients_http as http;
use skein_io as io;
use skein_lib::{Env, List, Map, Queue, Time, Token, Wall};
use temper_engine_domain as root;
pub mod bundle;
#[cfg(test)]
mod demo_tests;
pub mod fetch;
mod github;
pub mod google;
pub(crate) mod policy;
pub(crate) mod printed;
mod provider;
pub mod translation;
const PREFIX: u64 = crate::tokens::CLIENTS;
pub(crate) const DEMO_STARTS: u32 = 8;

#[expect(missing_debug_implementations, reason = "clients hold pending OAuth secrets")]
pub struct Clients {
    /// Bound address awaiting the system's note routing.
    pub(crate) listening: Option<::core::net::SocketAddr>,
    public: Option<crate::startup::application::Origin>,
    component: kit::Component,
    initial: Option<kit::Settings>,
    provider_documents: Option<provider::Documents>,
    provider_replies: Map<Token, provider::Reply>,
    next_provider: u64,
    server: crate::server::Server,
    mount: http::Mount,
    mount_env: Env<http::Limits>,
    mount_out: Queue<http::Request>,
    server_out: Queue<crate::server::Routed>,
    replies: Map<Token, bundle::Reply>,
    github: Map<Token, github::Reply>,
    pub(crate) google: Option<google::Google>,
    google_replies: Map<Token, google::Reply>,
    pub(crate) github_calls: Queue<(Token, skein_http::server::Call)>,
    pub(crate) query_bytes: u32,
    send: u32,
    bundle: Option<bundle::Bundle>,
    component_closed: bool,
    stopping: Option<()>,

    secret: Option<skein_lib::Secret>,
    pub(crate) env: Env<kit::Limits>,
    out: Queue<kit::Request>,
    pub(crate) io: Queue<io::Request>,
    pub(crate) links: Queue<Box<[u8]>>,
    pub(crate) secrets: Queue<kit::SecretsRequest>,
    pub(crate) closed: bool,
    pending: bool,
    pub(crate) starting: Option<()>,
    pub(crate) early: Queue<root::Event>,
    endings: Map<u64, Endings>,
    durable: u64,
    writes: u32,
    secret_owners: Map<Token, Asked>,
    next_secret: u64,
    demo_starts: Map<Token, root::Event>,
    demo_reads: Queue<crate::HandoverRead>,
    demo_ready: Queue<root::Event>,
    next_demo: u64,
    demo_carries: Queue<kit::SecretsRequest>,
}
/// What a secrets request in flight was for: a kept value's owner, or the
/// exchange whose carried value the store numbers itself.
#[derive(Clone, Copy, Debug)]
enum Asked {
    Owner(kit::SecretOwner),
    Carry(Token),
}
#[derive(Debug)]
struct Endings {
    numbers: List<u64>,
    next: u32,
}
fn invalid(what: What) -> Problems {
    let mut problems = Problems::empty();
    problems.push(Path::root().member(b"engine").member(b"clients"), what);
    problems
}
impl Clients {
    pub(crate) fn demo_read(&mut self) -> Option<crate::HandoverRead> {
        self.demo_reads.pop()
    }
    pub(crate) fn owns_demo_read(&self, run: Token) -> bool {
        self.demo_starts.contains_key(&run)
    }

    /// The service reads the file at each start and carries its bytes as a
    /// pasted value's (protocol/setup.md, 8.4). The shell's private read removes
    /// one final LF, as it does for a handover.
    pub(crate) fn demo_read_answer(
        &mut self,
        run: Token,
        answer: Result<Option<skein_lib::Secret>, crate::HandoverUnread>,
    ) {
        let value = match answer {
            Ok(Some(value)) if !value.expose().is_empty() && value.expose().len() <= 4096 => Some(value),
            Ok(Some(_) | None) | Err(_) => None,
        };
        if let Some(value) = value {
            if let Some(root::Event::Party(root::PartyInput::Application { carried, .. })) =
                self.demo_starts.get_mut(&run)
            {
                *carried = Some(core::Carried { number: 0, digest: skein_lib::sha256::digest(value.expose()) });
            }
            self.demo_carries.push(kit::SecretsRequest::Carry { exchange: run, value });
        } else {
            self.demo_carried(run, None);
        }
    }
    fn demo_carried(&mut self, run: Token, number: Option<u64>) {
        let Some(mut event) = self.demo_starts.remove(&run) else { return };
        if let root::Event::Party(root::PartyInput::Application { carried, .. }) = &mut event {
            match number {
                Some(number) => {
                    carried.as_mut().expect("carried demo digest").number = number;
                }
                None => *carried = None,
            }
        }
        self.demo_ready.push(event);
    }

    /// The live bound socket survives consumption of the listening notification.
    pub(crate) fn bound_address(&self) -> Option<::core::net::SocketAddr> {
        self.server.listening()
    }
    #[expect(clippy::too_many_lines, reason = "startup constructs bounded queues beside the client component")]
    #[expect(clippy::manual_map, reason = "the programming model uses explicit branches without closures")]
    pub fn new(
        config: &config::Engine,
        clients: &crate::startup::application::Clients,
        seeds: &Seeds,
        room: u32,
        journal: &root::JournalLimits,
        sign_in: Option<&crate::settings::SignIn>,
        files: &[(Box<[u8]>, skein_lib::Secret)],
    ) -> Result<Self, Problems> {
        let config::Address::Addr(listener) = &clients.listener else {
            return Err(invalid(What::Form));
        };
        let mut providers = List::with_capacity(u32::try_from(config.providers.len()).expect("bounded providers"));
        let mut link_provider = None;
        let mut deployment_provider = 0;
        for provider in &config.providers {
            match provider.kind {
                config::ProviderKind::Link => link_provider = Some(provider.number),
                config::ProviderKind::Deployment => deployment_provider = provider.number,
                config::ProviderKind::Application => {
                    providers.push(provider.number).expect("provider count");
                }
            }
        }
        let link = if config.clients.link {
            let provider = match sign_in {
                Some(sign_in) => sign_in.provider,
                None => link_provider.ok_or(invalid(What::Refers))?,
            };
            let mut identity = None;
            for owner in &config.owners {
                if owner.identity.provider == provider {
                    identity =
                        Some(people::Identity {
                            key: owner.identity.clone(),
                            login: match sign_in {
                                Some(sign_in) => google::member_login(sign_in, &owner.identity.subject)
                                    .ok_or(invalid(What::Refers))?,
                                None => Box::from(b"owner".as_slice()),
                            },
                            name: match sign_in {
                                Some(sign_in) => google::member_login(sign_in, &owner.identity.subject)
                                    .ok_or(invalid(What::Refers))?,
                                None => Box::from(b"Owner".as_slice()),
                            },
                        });
                    break;
                }
            }
            Some(identity.ok_or(invalid(What::Refers))?)
        } else {
            None
        };
        let mut served = List::with_capacity(2);
        served
            .push(kit::Served {
                family: None,
                first: jig_client_protocol_wire::VERSION,
                last: jig_client_protocol_wire::VERSION,
            })
            .or(Err(invalid(What::Bound)))?;
        served
            .push(kit::Served {
                family: Some(temper_client_protocol_wire::number(temper_client_protocol_wire::Owner::Forge)),
                first: temper_client_protocol_wire::CLIENT_VERSION,
                last: temper_client_protocol_wire::CLIENT_VERSION,
            })
            .or(Err(invalid(What::Bound)))?;
        // The provider owns its signer; construct the initial settings twice without cloning it.
        let settings = || -> Result<kit::Settings, Problems> {
            let provider = match &config.clients.provider {
                Some(provider) => match config::clients::provider_issuer(
                    provider,
                    files,
                    provider::urls(clients, *listener)?,
                    &config.limits.clients,
                ) {
                    Ok(issuer) => Some(issuer),
                    Err(problem) => return Err(provider::problem(problem)),
                },
                None => None,
            };
            Ok(kit::Settings {
                provider,
                addresses: match &config.clients.provider {
                    Some(provider) => provider.addresses.clone(),
                    None => Box::new([]),
                },
                ..client_settings(config, seeds, link.clone(), served.clone(), providers.clone(), deployment_provider)
            })
        };
        let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: config.limits.clients };
        let mut out = Queue::with_capacity(room);
        let initial = settings()?;
        let provider_documents = match &initial.provider {
            Some(issuer) => Some(provider::Documents { discovery: issuer.discovery(), keys: issuer.key_set() }),
            None => None,
        };
        let component = kit::Component::new(initial, &env, &mut out).or(Err(invalid(What::Bound)))?;
        Ok(Self {
            listening: None,
            public: clients.public.clone(),
            component,
            initial: Some(settings()?),
            provider_documents,
            provider_replies: Map::with_capacity(crate::limits::server(&config.limits).connections),
            next_provider: 1,
            server: crate::server::Server::new(*listener, &crate::limits::server(&config.limits)),
            mount: http::Mount::new(http_settings(clients.public.as_ref()), &config.limits.clients_http),
            mount_env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits: config.limits.clients_http },
            mount_out: Queue::with_capacity(room),
            server_out: Queue::with_capacity(room),
            replies: Map::with_capacity(crate::limits::server(&config.limits).connections),
            google: None,
            google_replies: Map::with_capacity(crate::limits::server(&config.limits).connections),
            github: Map::with_capacity(crate::limits::server(&config.limits).connections),
            github_calls: Queue::with_capacity(crate::limits::server(&config.limits).connections),
            query_bytes: 2048,
            send: crate::limits::server(&config.limits).http.send,
            bundle: None,
            component_closed: false,
            stopping: None,

            secret: None,
            env,
            out,
            io: Queue::with_capacity(room),
            links: Queue::with_capacity(room),
            secrets: Queue::with_capacity(room),
            closed: false,
            pending: true,
            starting: Some(()),
            early: Queue::with_capacity(room),
            endings: Map::with_capacity(journal.commits),
            secret_owners: Map::with_capacity(room),
            next_secret: 1,
            demo_starts: Map::with_capacity(DEMO_STARTS),
            demo_reads: Queue::with_capacity(DEMO_STARTS),
            demo_ready: Queue::with_capacity(DEMO_STARTS),
            next_demo: 1,
            demo_carries: Queue::with_capacity(DEMO_STARTS),
            durable: 0,
            writes: journal.writes,
        })
    }
    /// Hold the bundle the binary read at start.
    pub(crate) fn adopt_bundle(&mut self, bundle: bundle::Bundle) {
        self.bundle = Some(bundle);
    }
    fn print_link(&mut self) {
        if let Some(listener) = self.server.listening()
            && let Some(secret) = self.secret.take()
        {
            self.links.push(printed::link_public(listener, self.public.as_ref(), &secret));
        }
    }
    #[must_use]
    pub fn room(&self, batch: u32) -> bool {
        let count = batch.checked_mul(kit::MAX_OUT_DOWN).expect("client routing room");
        self.out.room() >= count
            && self.io.room() >= count
            && self.links.room() >= count
            && self.secrets.room() >= count
    }
    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        crate::accounts::earliest(
            crate::accounts::earliest(self.component.next_deadline(), self.server.next_deadline()),
            match &self.google {
                Some(google) => google.next_deadline(),
                None => None,
            },
        )
    }
    #[must_use]
    pub fn work_pending(&self, now: Time) -> bool {
        !self.closed
            && ((match &self.google {
                Some(google) => google.work_pending(),
                None => false,
            }) || self.pending
                || !self.secrets.is_empty()
                || self.endings_ready()
                || !self.demo_carries.is_empty()
                || !self.demo_ready.is_empty()
                || !self.out.is_empty()
                || !self.early.is_empty()
                || !self.github_calls.is_empty()
                || !self.io.is_empty()
                || !self.links.is_empty()
                || match self.next_deadline() {
                    Some(at) => at <= now,
                    None => false,
                })
    }
    pub fn down(&mut self, event: kit::Event) {
        if let kit::Event::Stop | kit::Event::Close | kit::Event::Abort = &event {
            self.stopping = Some(());
            self.initial = None;
        }
        if let kit::Event::Stop = &event {
            self.server.stop(&mut self.server_out);
        }
        if let kit::Event::Close = &event {
            self.server.close(&mut self.server_out);
        }
        if let kit::Event::Abort = &event {
            self.server.abort(&mut self.server_out);
        }
        if !self.component_closed {
            self.component.down(&self.env, event, &mut self.out);
            self.pending = true;
        }
    }
    pub(crate) fn abort(&mut self, unstarted: bool) {
        self.down(kit::Event::Abort);
        if unstarted {
            self.io = Queue::with_capacity(self.io.capacity());
            self.closed = true;
        }
    }
    #[must_use]
    pub fn opened(&self) -> bool {
        self.server.opened()
    }
    pub fn up(&mut self, event: io::Event) -> bool {
        let owner = event_owner(&event);
        if let Some(owner) = owner
            && owner.raw() & (1_u64 << 59_u32) != 0
        {
            self.google_up(event);
            return true;
        }
        let event = untag(event);
        let mut failed = false;
        if let io::Event::Failed { owner, .. } | io::Event::Closed { owner } = &event
            && *owner == Token::new(0)
            && self.stopping.is_none()
        {
            failed = true;
        }
        if let io::Event::Listening { addr, .. } = &event
            && let Some(address) = addr.socket()
        {
            let mut hosts = List::with_capacity(2);
            if let Some(origin) = &self.public {
                hosts.push(origin.name.clone()).expect("public host");
            }
            hosts.push(printed::address_text(address).expect("bound listener address")).expect("one listener host");
            self.mount.down(&self.mount_env, http::Event::Listening { hosts }, &mut self.mount_out);
        }
        let previous = self.server.listening();
        // The server must observe the terminal bind event before the engine
        // aborts, or it waits forever for the listener to finish opening.
        self.server.up(event, &mut self.server_out);
        if self.server.listening() != previous {
            self.listening = self.server.listening();
        }
        self.print_link();
        self.pending = true;
        !failed
    }
    fn server_requests(&mut self) {
        for _ in 0..self.server_out.len() {
            if self.io.room() == 0 || self.out.room() < kit::MAX_OUT_DOWN {
                break;
            }
            match self.server_out.pop().expect("server requests") {
                crate::server::Routed::Io(request) => self.io.push(tag(request)),
                crate::server::Routed::Call { call, head } => {
                    // Claims depend on the prefix alone; the mounted mapping checks the Host.
                    let claimed = http::claims(&http_settings(None), &head);
                    if claimed && self.mount.room() > 0 {
                        self.mount.down(&self.mount_env, http::Event::Call { call, head }, &mut self.mount_out);
                    } else if !claimed && provider::claims(&head.target) {
                        self.provider_call(call, head);
                    } else if !claimed && google::claims(&head.target) {
                        self.google_call(call, head);
                    } else if !claimed && github::claims(&head.target) {
                        self.github_call(call, head);
                    } else {
                        self.serve_call(call, &head, claimed);
                    }
                }
                crate::server::Routed::Http { call, event } => {
                    if self.provider_replies.contains_key(&call) {
                        self.provider_event(call, event);
                    } else if self.google_replies.contains_key(&call) {
                        self.google_event(call, event);
                    } else if self.github.contains_key(&call) {
                        self.github_event(call, event);
                    } else if self.replies.get(&call).is_some() {
                        self.serve_event(call, event);
                    } else {
                        self.mount.down(&self.mount_env, http::Event::Http { call, event }, &mut self.mount_out);
                    }
                }
            }
        }
        self.closed = self.component_closed
            && self.server.closed()
            && match &self.google {
                Some(google) => google.closed(),
                None => true,
            };
    }
    fn root(&mut self, value: root::Event, events: &mut Queue<root::Event>) {
        if self.starting.is_some() {
            self.early.push(value);
        } else {
            events.push(value);
        }
    }
    pub fn advance(
        &mut self,
        events: &mut Queue<root::Event>,
        forges: &[crate::settings::Forge],
        demo: Option<&crate::settings::Demo>,
        accounts: &mut Option<crate::accounts::Accounts>,
    ) {
        for _ in 0..self.demo_ready.len() {
            if events.room() == 0 || (self.starting.is_some() && self.early.room() == 0) {
                break;
            }
            let value = self.demo_ready.pop().expect("ready demo start");
            self.root(value, events);
        }
        for _ in 0..self.out.len() {
            if self.io.room() == 0
                || self.demo_starts.len().saturating_add(self.demo_ready.len()) >= DEMO_STARTS
                || self.links.room() == 0
                || self.secrets.room() == 0
                || (self.starting.is_some() && self.early.room() == 0)
                || (self.starting.is_none() && events.room() == 0)
            {
                break;
            }
            match self.out.pop().expect("client requests") {
                kit::Request::Transport(message) => {
                    self.mount.down(&self.mount_env, http::Event::Component(message), &mut self.mount_out);
                }
                kit::Request::Link { secret } => {
                    self.secret = Some(secret);
                    self.print_link();
                }
                kit::Request::TakeMoved { exchange, source } => {
                    let record = match accounts {
                        Some(accounts) => accounts.take_moved(source),
                        None => None,
                    };
                    self.down(kit::Event::MovedTaken { exchange, record });
                }
                kit::Request::Secrets(value) => self.secrets.push(value),
                kit::Request::Core(value) => match translation::to_root(*value) {
                    Ok(value) => self.root(root::Event::Party(value), events),
                    Err(value) => self.down(*value),
                },
                kit::Request::Application(value) => {
                    match crate::family::to_root(value, forges, demo, &temper_client_protocol_wire::v1::CEILINGS) {
                        Ok(mut value) => {
                            if let root::Event::Party(root::PartyInput::Application {
                                request: root::Application::Forge(temper_engine_domain_forge::Ask::StartDemo),
                                ..
                            }) = &value
                                && let Some(demo) = demo
                            {
                                if let root::Event::Party(root::PartyInput::Application { carried, .. }) = &mut value
                                    && let Some(carried) = carried.take()
                                {
                                    self.down(kit::Event::Application(kit::FromApplication::Unused {
                                        number: carried.number,
                                    }));
                                }
                                let run = Token::new((1_u64 << 63) | self.next_demo);
                                self.next_demo = self.next_demo.checked_add(1).expect("demo read sequence");
                                self.demo_starts.insert(run, value).expect("bounded parked starts");
                                self.demo_reads.push(crate::HandoverRead {
                                    forge: demo.forge,
                                    run,
                                    path: demo.key_file.clone(),
                                    max: 4097,
                                });
                            } else {
                                self.root(value, events);
                            }
                        }
                        Err(value) => self.down(*value),
                    }
                }
                kit::Request::Minted { exchange, credential, expires } => {
                    self.google_minted(exchange, Some((credential, expires)));
                }
                kit::Request::MintRefused { exchange, .. } => self.google_minted(exchange, None),
                kit::Request::Redeemed { exchange, answer } | kit::Request::Claimed { exchange, answer } => {
                    self.provider_answer(exchange, answer);
                }
                kit::Request::Closed => self.component_closed = true,
            }
        }
    }
    pub fn clock(&mut self, now: Time, wall: Wall) {
        self.env.now = now;
        self.env.wall = wall;
        self.mount_env.now = now;
        self.mount_env.wall = wall;
        self.server.clock(now, wall);
    }
    pub fn iterate(
        &mut self,
        now: Time,
        wall: Wall,
        events: &mut Queue<root::Event>,
        forges: &[crate::settings::Forge],
        demo: Option<&crate::settings::Demo>,
        accounts: &mut Option<crate::accounts::Accounts>,
    ) {
        self.clock(now, wall);
        if let Some(settings) = self.initial.take() {
            // Link lifetimes start on the supplied clock, which need not start at zero.
            self.out = Queue::with_capacity(self.out.capacity());
            self.component =
                kit::Component::new(settings, &self.env, &mut self.out).expect("validated client settings");
        }
        if self.closed || !self.room(1) || self.early.room() < kit::MAX_OUT_DOWN || events.room() < kit::MAX_OUT_DOWN {
            return;
        }
        self.server.fire(&mut self.server_out);
        let server_pending = self.server.advance(&mut self.server_out);
        self.server_requests();
        self.google_advance(events);
        self.endings_step(events, forges, demo, accounts);
        self.advance(events, forges, demo, accounts);
        if self.closed {
            return;
        }
        if !self.component_closed {
            self.component.fire(&self.env, &mut self.out);
        }
        self.advance(events, forges, demo, accounts);
        self.pending = server_pending || !self.server_out.is_empty();
        let mount_pending = self.mount.up(&self.mount_env, &mut self.mount_out);
        self.pending |= mount_pending;
        if self.component_closed && !mount_pending && self.mount_out.is_empty() && self.out.is_empty() {
            self.server.finish();
            self.pending = true;
        }
        if let Some(request) = self.mount_out.pop() {
            self.pending = true;
            match request {
                http::Request::Component(message) => self.down(kit::Event::Transport(message)),
                http::Request::Http { call, request } => self.server.answer(call, request, &mut self.server_out),
            }
        }
        if !self.component_closed {
            self.pending |= self.component.up(&self.env, &mut self.out);
        }

        self.advance(events, forges, demo, accounts);
    }
}
#[must_use]
pub fn owns(token: Token) -> bool {
    token.raw() & (15_u64 << 60) == crate::tokens::SERVER
}
#[must_use]
pub fn tag(mut request: io::Request) -> io::Request {
    match &mut request {
        io::Request::Listen { owner, .. }
        | io::Request::Connect { owner, .. }
        | io::Request::Bind { owner, .. }
        | io::Request::Spawn { owner, .. }
        | io::Request::Usage { owner }
        | io::Request::Window { owner, .. } => {
            assert!(owner.raw() < (1_u64 << 60_u32), "bounded client exchange names");
            *owner = Token::new(owner.raw() | crate::tokens::SERVER);
        }
        io::Request::Reject { .. }
        | io::Request::Stream { .. }
        | io::Request::Output { .. }
        | io::Request::Signal { .. }
        | io::Request::Close { .. }
        | io::Request::Abort { .. } => {}
    }
    request
}
#[must_use]
pub fn untag(mut event: io::Event) -> io::Event {
    match &mut event {
        io::Event::Listening { owner, .. }
        | io::Event::Accepted { owner, .. }
        | io::Event::Connecting { owner, .. }
        | io::Event::Connected { owner }
        | io::Event::Stream { owner, .. }
        | io::Event::Output { owner, .. }
        | io::Event::Spawned { owner, .. }
        | io::Event::Exited { owner, .. }
        | io::Event::Usage { owner, .. }
        | io::Event::Window { owner, .. }
        | io::Event::Failed { owner, .. }
        | io::Event::Closed { owner } => {
            *owner = Token::new(owner.raw() & !(15_u64 << 60));
        }
        io::Event::Resized | io::Event::Shutdown { .. } => {}
    }
    event
}

#[must_use]
pub fn worst_case(
    limits: &kit::Limits,
    transport: &crate::server::Limits,
    mapping: &http::Limits,
    room: u32,
    journal: &root::JournalLimits,
) -> Option<u64> {
    kit::worst_case(limits)?
        .checked_add(Map::<Token, root::Event>::worst_case(DEMO_STARTS)?)?
        .checked_add(Queue::<root::Event>::worst_case(DEMO_STARTS)?)?
        .checked_add(Queue::<crate::HandoverRead>::worst_case(DEMO_STARTS)?)?
        .checked_add(Queue::<kit::SecretsRequest>::worst_case(DEMO_STARTS)?)?
        .checked_add(u64::from(DEMO_STARTS).checked_mul(4097)?)?
        .checked_add(u64::try_from(bundle::NO_BUNDLE_PAGE.len()).ok()?)?
        // First-clock initialization briefly owns both validated component instances.
        .checked_add(kit::worst_case(limits)?)?
        .checked_add(http::worst_case(mapping)?)?
        .checked_add(Map::<Token, bundle::Reply>::worst_case(transport.connections)?)?
        // Current query decoding and measured page construction, before ownership moves.
        .checked_add(u64::from(transport.http.head).checked_add(4096)?)?
        .checked_add(provider::worst_case(limits, transport)?)?
        .checked_add(Map::<Token, github::Reply>::worst_case(transport.connections)?)?
        .checked_add(Queue::<(Token, skein_http::server::Call)>::worst_case(transport.connections)?)?
        .checked_add(
            u64::from(transport.connections).checked_mul(
                u64::from(transport.http.head).checked_add(4096)?.checked_add(
                    u64::from(transport.http.headers)
                        .checked_mul(u64::try_from(::core::mem::size_of::<skein_http::Header>()).ok()?)?,
                )?,
            )?,
        )?
        .checked_add(u64::from(transport.connections).checked_mul(u64::from(transport.http.head))?)?
        .checked_add(crate::server::worst_case(transport)?)?
        .checked_add(Queue::<crate::server::Routed>::worst_case(room)?)?
        .checked_add(Queue::<http::Request>::worst_case(room)?)?
        .checked_add(List::<Box<[u8]>>::worst_case(1)?.checked_add(64)?)?
        .checked_add(Queue::<kit::Request>::worst_case(room)?)?
        .checked_add(Queue::<io::Request>::worst_case(room)?)?
        .checked_add(Queue::<root::Event>::worst_case(room)?)?
        .checked_add(u64::from(limits.message_bytes).checked_mul(64)?.checked_mul(u64::from(room).checked_mul(2)?)?)?
        .checked_add(u64::from(transport.http.send).checked_mul(u64::from(room))?)?
        .checked_add(Queue::<Box<[u8]>>::worst_case(room)?)?
        .checked_add(u64::from(room).checked_mul(512)?)?
        .checked_add(Queue::<kit::SecretsRequest>::worst_case(room)?)?
        .checked_add(Map::<Token, Asked>::worst_case(room)?)?
        .checked_add(u64::from(room).checked_mul(40)?)?
        // A carried value waits here on its way to the store.
        .checked_add(u64::from(room).checked_mul(u64::from(limits.wire.keyed_value_some))?)?
        .checked_add(Map::<u64, Endings>::worst_case(journal.commits)?)?
        .checked_add(List::<u64>::worst_case(journal.writes)?.checked_mul(u64::from(journal.commits))?)
}

impl Clients {
    pub fn commit(&mut self, number: u64, writes: &[root::Write]) {
        let mut endings = List::with_capacity(self.writes);
        for write in writes {
            match write {
                root::Write::Erase(key) => {
                    if let Some(number) = ended(key)
                        && !endings.as_slice().contains(&number)
                    {
                        endings.push(number).expect("bounded commit writes");
                    }
                }
                root::Write::CopySettingsRevision(_) | root::Write::Save(_) => {}
            }
        }
        if !endings.is_empty() {
            self.endings.insert(number, Endings { numbers: endings, next: 0 }).expect("bounded commits in flight");
        }
    }
    pub fn committed(&mut self, number: u64) {
        self.durable = self.durable.max(number);
    }
    #[must_use]
    pub fn commit_room(&self) -> bool {
        self.endings.len() < self.endings.capacity()
    }
    fn endings_ready(&self) -> bool {
        for (number, _) in &self.endings {
            if *number <= self.durable {
                return true;
            }
        }
        false
    }
    fn endings_step(
        &mut self,
        events: &mut Queue<root::Event>,
        forges: &[crate::settings::Forge],
        demo: Option<&crate::settings::Demo>,
        accounts: &mut Option<crate::accounts::Accounts>,
    ) {
        for _ in 0..self.endings.len().checked_mul(self.writes).expect("bounded sign-in endings") {
            if !self.room(1) || events.room() < kit::MAX_OUT_DOWN {
                break;
            }
            let mut ready = None;
            for (number, _) in &self.endings {
                if *number <= self.durable {
                    ready = Some(*number);
                    break;
                }
            }
            let Some(number) = ready else { return };
            let endings = self.endings.get_mut(&number).expect("durable endings");
            let sign_in = *endings.numbers.get(endings.next).expect("one sign-in per erased key");
            endings.next = endings.next.checked_add(1).expect("bounded erase index");
            if endings.next == endings.numbers.len() {
                self.endings.remove(&number);
            }
            self.down(kit::Event::SignInEnded { sign_in });
            self.advance(events, forges, demo, accounts);
        }
    }
}

fn ended(key: &root::Key) -> Option<u64> {
    match key {
        root::Key::Core(core::Key::People(people::Key::SignIn(number))) => Some(*number),
        root::Key::Core(
            core::Key::People(_)
            | core::Key::Core(_)
            | core::Key::Tasks(_)
            | core::Key::Notes(_)
            | core::Key::Accounts(_),
        )
        | root::Key::Forge(_) => None,
    }
}

impl Clients {
    pub fn not_ready(&mut self, value: root::PartyInput) {
        let mut unused = None;
        if let root::PartyInput::Key { carried, .. } = &value {
            unused = Some(carried);
        }
        if let root::PartyInput::Application { carried, .. } = &value {
            unused = carried.as_ref();
        }
        let carried = unused;
        if let Some(carried) = carried {
            self.down(kit::Event::Application(kit::FromApplication::Unused { number: carried.number }));
        }
        if let Some(event) = translation::not_ready(value) {
            self.down(event);
        }
    }
}

impl Clients {
    pub fn secrets_step(&mut self, store: &mut crate::store::Store) {
        for _ in 0..store.secrets.len() {
            if self.out.room() < kit::MAX_OUT_DOWN {
                break;
            }
            let answer = store.secrets.pop().expect("secrets terminals");
            self.secret_answer(answer);
        }
        self.advance_secrets(store);
    }
    fn advance_secrets(&mut self, store: &mut crate::store::Store) {
        for _ in 0..self.demo_carries.len() {
            if self.secrets.room() == 0 {
                break;
            }
            let request = self.demo_carries.pop().expect("pending demo carries");
            self.secrets.push(request);
        }
        for _ in 0..self.secrets.len() {
            if self.out.room() < kit::MAX_OUT_DOWN
                || !store.room()
                || self.secret_owners.len() == self.secret_owners.capacity()
            {
                break;
            }
            let request = self.secrets.pop().expect("client secrets requests");
            let (owner, request) = secret_request(request);
            let token = Token::new(PREFIX.checked_add(self.next_secret).expect("secret routing name"));
            self.next_secret = self.next_secret.checked_add(1).expect("secret routing sequence");
            self.secret_owners.insert(token, owner).expect("bounded secrets in flight");
            if let Some(answer) = store.secrets(token, request) {
                self.secret_answer(answer);
            }
        }
    }
    fn secret_answer(&mut self, answer: jig_protocol_store::Event) {
        use jig_protocol_store::Event;
        let event = match answer {
            Event::Kept { owner } => kit::SecretsAnswer::Kept { owner: self.owner(owner) },
            Event::Erased { owner } => kit::SecretsAnswer::Erased { owner: self.owner(owner) },
            Event::Carried { owner, number } => {
                let exchange = self.carry(owner);
                if self.owns_demo_read(exchange) {
                    self.demo_carried(exchange, Some(number));
                    return;
                }
                kit::SecretsAnswer::Carried { exchange, number: Some(number) }
            }
            Event::Stale { owner } => match self.secret_owners.remove(&owner).expect("secret stale owner") {
                Asked::Carry(exchange) => {
                    if self.owns_demo_read(exchange) {
                        self.demo_carried(exchange, None);
                        return;
                    }
                    kit::SecretsAnswer::Carried { exchange, number: None }
                }
                Asked::Owner(_) => kit::SecretsAnswer::Failed,
            },
            Event::Secret { owner, value } => {
                let owner = self.owner(owner);
                let value = match value {
                    Some(value) => match kept(value) {
                        Some(value) => Some(value),
                        None => {
                            self.down(kit::Event::Secrets(kit::SecretsAnswer::Failed));
                            return;
                        }
                    },
                    None => None,
                };
                kit::SecretsAnswer::Got { owner, value }
            }
            Event::SecretsLoaded { .. }
            | Event::Site { .. }
            | Event::Patched { .. }
            | Event::Opened { .. }
            | Event::OpenRefused { .. }
            | Event::Committed { .. }
            | Event::Failed { .. }
            | Event::Restored { .. }
            | Event::Loaded { .. }
            | Event::Read { .. }
            | Event::Got { .. }
            | Event::Turns { .. }
            | Event::Unread { .. }
            | Event::Corrupt { .. } => kit::SecretsAnswer::Failed,
        };
        self.down(kit::Event::Secrets(event));
    }
    fn owner(&mut self, token: Token) -> kit::SecretOwner {
        match self.secret_owners.remove(&token).expect("secret request owner") {
            Asked::Owner(owner) => owner,
            Asked::Carry(_) => unreachable!("the store answers a carry Carried or Stale"),
        }
    }
    fn carry(&mut self, token: Token) -> Token {
        match self.secret_owners.remove(&token).expect("secret carry owner") {
            Asked::Carry(exchange) => exchange,
            Asked::Owner(_) => unreachable!("the store answers Carried only to a carry"),
        }
    }
}
fn secret_request(request: kit::SecretsRequest) -> (Asked, jig_protocol_store::Secrets) {
    use jig_protocol_store::Secrets;
    match request {
        kit::SecretsRequest::Keep { owner, value } => {
            let (kind, number) = place(owner);
            let mut writer = skein_lib::Writer::new(40);
            writer.put(&value.expires.as_nanos().to_be_bytes()).expect("expiry bytes");
            writer.put(value.digest.expose()).expect("32-byte digest");
            (Asked::Owner(owner), Secrets::Keep { owner: kind, number, value: skein_lib::Secret::new(writer.finish()) })
        }
        kit::SecretsRequest::Erase { owner } => {
            let (kind, number) = place(owner);
            (Asked::Owner(owner), Secrets::Erase { owner: kind, number })
        }
        kit::SecretsRequest::Get { owner } => {
            let (kind, number) = place(owner);
            (Asked::Owner(owner), Secrets::Get { owner: kind, number })
        }
        kit::SecretsRequest::Carry { exchange, value } => (Asked::Carry(exchange), Secrets::Carry { value }),
        kit::SecretsRequest::Hold { exchange, value } => (Asked::Carry(exchange), Secrets::Hold { value }),
    }
}
/// Where the store keeps an owner's value; a carried value is the
/// application's, under the number the store drew (jig's store.md 9).
fn place(owner: kit::SecretOwner) -> (jig_protocol_store::SecretOwner, u64) {
    use jig_protocol_store::SecretOwner;
    match owner {
        kit::SecretOwner::SignIn(number) => (SecretOwner::SignIn, number),
        kit::SecretOwner::Service(number) => (SecretOwner::Service, number),
        kit::SecretOwner::Carried(number) => (SecretOwner::Application, number),
    }
}
fn kept(value: skein_lib::Secret) -> Option<kit::Kept> {
    let bytes = value.expose();
    if bytes.len() != 40 {
        return None;
    }
    let expires = u64::from_be_bytes(bytes.get(..8)?.try_into().ok()?);
    Some(kit::Kept { digest: skein_lib::Secret::new(Box::from(bytes.get(8..)?)), expires: Wall::from_nanos(expires) })
}

#[expect(clippy::manual_map, reason = "the strict subset does not use closures")]
fn http_settings(public: Option<&crate::startup::application::Origin>) -> http::Settings {
    let public = match public {
        Some(origin) => Some(origin.name.clone()),
        None => None,
    };
    http::Settings { prefix: Box::from(b"/jig".as_slice()), public }
}

fn event_owner(event: &io::Event) -> Option<Token> {
    match event {
        io::Event::Listening { owner, .. }
        | io::Event::Accepted { owner, .. }
        | io::Event::Connecting { owner, .. }
        | io::Event::Connected { owner }
        | io::Event::Stream { owner, .. }
        | io::Event::Output { owner, .. }
        | io::Event::Spawned { owner, .. }
        | io::Event::Exited { owner, .. }
        | io::Event::Usage { owner, .. }
        | io::Event::Window { owner, .. }
        | io::Event::Failed { owner, .. }
        | io::Event::Closed { owner } => Some(*owner),
        io::Event::Resized | io::Event::Shutdown { .. } => None,
    }
}

fn client_settings(
    config: &config::Engine,
    seeds: &Seeds,
    link: Option<people::Identity>,
    served: List<kit::Served>,
    providers: List<u16>,
    deployment_provider: u16,
) -> kit::Settings {
    kit::Settings {
        provider: None,
        addresses: Box::new([]),
        link,
        deployment_provider,
        heartbeat: config.clients.heartbeat,
        rows: config.limits.core.documents.rows,
        served,
        providers,
        seed: seeds.crypto.clone(),
        stream: seeds.seed,
        models: config::clients::models(&config.llm),
        key_bytes: config.limits.driver.client.document.token_bytes.min(config.limits.clients.wire.keyed_value_some),
        redirect_bytes: config.limits.driver.client.uri_bytes,
        moved_bytes: moved_bytes(&config.limits),
    }
}

pub(crate) fn moved_bytes(limits: &config::Limits) -> u64 {
    jig_protocol_accounts::moved::bound(&limits.driver.client.document)
}
