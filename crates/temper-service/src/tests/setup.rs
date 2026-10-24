use super::*;
use crate::tests::fixtures;
use skein_forge_setup::github::manifest::Target;
use skein_lib::{List, Rng};
use temper_engine_domain_forge as domain;
use temper_protocol_records as records;

fn service(states: u32, life: Duration) -> crate::Service {
    let mut service = routing_service(states, life);
    let ready = fixtures::ready().engine.expect("engine");
    service.engine.as_mut().expect("engine").env.limits = ready.env.limits;
    service.engine.as_mut().expect("engine").domain = ready.domain;
    service
}
// The page owners emit their measured sizes only in the usual figure write mode.
#[expect(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    reason = "figure write mode names and writes the measured page sizes"
)]
pub(super) fn measure_page(name: &str, bytes: usize) {
    if std::env::var_os("SKEIN_CODEGEN_WRITE").is_some_and(|value| value == "1") {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/q-pages");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join(format!("{name}.txt")), format!("{bytes}\n")).unwrap();
    }
}
// Token routing observes queued root inputs, so it needs no second service's restarted root.
pub(super) fn routing_service(states: u32, life: Duration) -> crate::Service {
    routing_service_at(states, life, None)
}
fn routing_service_at(
    states: u32,
    life: Duration,
    public: Option<crate::startup::application::Origin>,
) -> crate::Service {
    let config = github_config(states, life, public);
    let mut runtime = List::with_capacity(1);
    runtime
        .push(crate::ForgeRuntime {
            number: 1,
            address: ([127, 0, 0, 1], 3000).into(),
            transport: skein_forge_connection::settings::Transport::Plaintext { name: None },
            trust_roots: 0,
            trust_root_bytes: 0,
        })
        .unwrap();
    let mut service = crate::Service::engine(config, &fixtures::seeds(), None, runtime).unwrap();
    service.engine.as_mut().expect("engine").store = fixtures::records_store();
    load_routing(&mut service);
    service
}
fn github_config(
    states: u32,
    life: Duration,
    public: Option<crate::startup::application::Origin>,
) -> crate::settings::Engine {
    let mut config = fixtures::engine();
    config.clients.public = public;
    config.forges[0].kind = crate::settings::Kind::GitHub;
    config.identities[0].token = Secret::new(Box::new([]));
    config.identities[0].login = Box::new([]);
    config.identities[0].author.name = Box::new([]);
    config.identities[0].author.email = Box::new([]);
    config.forges[0].api = jig_protocol_config::Address::Addr(skein_io::Addr::Ip(([127, 0, 0, 1], 3000).into()));
    config.forges[0].web = Some(Box::from(b"http://127.0.0.1:3000".as_slice()));
    config.forges[0].api_token = None;
    config.forges[0].transport = crate::settings::Transport::Plaintext;
    config.limits.setup.states = states;
    config.limits.setup.state_life = life;
    fixtures::fill_files(&mut config);
    config
}
fn load_routing(service: &mut crate::Service) {
    assert!(service.engine.as_ref().expect("engine").setup.component.is_none(), "no host before the rows load");
    for _ in 0_u32..8 {
        crate::forge::records::advance(service.engine.as_mut().expect("engine"));
    }
    advance(service.engine.as_mut().expect("engine"));
    assert!(service.engine.as_ref().expect("engine").setup.component.is_some());
    assert!(service.engine.as_ref().expect("engine").forge_records.done());
    for _ in 0..service.engine.as_mut().expect("engine").notes.len() {
        let _startup_note = service.engine.as_mut().expect("engine").notes.pop();
    }
    // These routing tests fake the listeners, leaving the host and forge as their real components.
    for _ in 0..service.engine.as_mut().expect("engine").workers.io.len() {
        let skein_io::Request::Listen { owner, addr } =
            service.engine.as_mut().expect("engine").workers.io.pop().unwrap()
        else {
            panic!("worker listener")
        };
        let engine = service.engine.as_mut().expect("engine");
        assert!(engine.workers.up(
            Time::ZERO,
            skein_io::Event::Listening { owner, listener: Token::new(78), addr },
            &mut engine.events
        ));
    }
}

pub(super) fn start(service: &mut crate::Service, run: u64, plan: host::Plan) {
    let run = Token::new(run);
    service.engine.as_mut().expect("engine").setup.staged.insert(run, plan).unwrap();
    crate::route::request(service.engine.as_mut().expect("engine"), root::Request::Setup { run });
}
pub(super) fn terminal(service: &mut crate::Service) -> root::SetupEnd {
    for _ in 0_u32..4 {
        advance(service.engine.as_mut().expect("engine"));
    }
    for _ in 0..service.engine.as_mut().expect("engine").events.len() {
        if let Some(root::Event::Setup { end, .. }) = service.engine.as_mut().expect("engine").events.pop() {
            return end;
        }
    }
    panic!("setup terminal")
}
fn manifest(service: &mut crate::Service, run: u64) -> Box<[u8]> {
    start(service, run, host::Plan::Manifest { forge: 1, target: Target::Personal });
    let root::SetupEnd::Done { outcome: root::SetupOutcome::Manifest { state }, .. } = terminal(service) else {
        panic!("manifest")
    };
    state
}
fn row(service: &mut crate::Service, item: Item, bytes: &[u8]) {
    service.engine.as_mut().expect("engine").forge_records.rows.push(
        records::number(1, item),
        Secret::new(records::encode(bytes).into_boxed_slice()),
        4096,
    );
}
#[test]
fn the_host_starts_after_loading_with_temper_urls_and_the_configured_limits() {
    let mut service = service(8, Duration::from_secs(3600));
    let state = manifest(&mut service, 1);
    assert_eq!(u64::try_from(state.len()).unwrap(), STATE_TEXT_BYTES);
    let page = service.setup_page(1, &state, Time::ZERO).unwrap();
    for expected in [
        b"temper-".as_slice(),
        b"/forges/1/github/redirect",
        b"/forges/1/github/setup",
        b"http://127.0.0.1:3000/settings/apps/new",
    ] {
        assert!(contains(page, expected));
    }
    assert_eq!(service.engine.as_ref().expect("engine").setup.env.limits.runs, 8);
    assert_eq!(service.engine.as_ref().expect("engine").setup.env.limits.states, 8);
    assert!(service.engine.as_ref().expect("engine").setup.io.is_empty(), "manifest is local");
}
#[test]
#[expect(clippy::manual_map, reason = "the step subset uses explicit matches")]
fn reads_use_the_retained_row_past_its_version_and_absence_answers_none() {
    for present in [false, true] {
        let mut service = service(8, Duration::from_secs(3600));
        if present {
            row(&mut service, Item::AppSlug, b"app-fixture");
        }
        start(&mut service, 1, host::Plan::Install { forge: 1 });
        let end = terminal(&mut service);
        let (_, host::Answer::Read(value)) = service.engine.as_mut().expect("engine").setup.answers.pop().unwrap()
        else {
            panic!("read answer")
        };
        assert_eq!(
            match &value {
                Some(value) => Some(value.expose().len()),
                None => None,
            },
            if present { Some(11) } else { None }
        );
        match end {
            root::SetupEnd::Done { outcome: root::SetupOutcome::Install { url, .. }, .. } => {
                assert!(url.starts_with(b"http://127.0.0.1:3000/apps/app-fixture/"));
            }
            root::SetupEnd::Stopped { reason, .. } => assert!(reason.ends_with(b"Missing")),
            root::SetupEnd::Done { .. } => panic!("install terminal"),
        }
        assert!(service.engine.as_ref().expect("engine").setup.io.is_empty());
    }
}
#[test]
fn pages_expire_on_their_done_clock_and_reload_does_not_consume_them() {
    let mut service = service(2, Duration::from_secs(3));
    let state = manifest(&mut service, 1);
    for seconds in [0, 2, 2] {
        assert!(service.setup_page(1, &state, Time::ZERO.saturating_add(Duration::from_secs(seconds))).is_some());
    }
    assert!(service.setup_page(2, &state, Time::ZERO).is_none());
    assert!(service.setup_page(1, &state, Time::ZERO.saturating_add(Duration::from_secs(3))).is_none());
    service.engine.as_mut().expect("engine").env.now = Time::ZERO.saturating_add(Duration::from_secs(3));
    advance(service.engine.as_mut().expect("engine"));
    assert!(service.engine.as_ref().expect("engine").setup.pages.is_empty());
    assert_eq!(service.engine.as_ref().expect("engine").setup.next_deadline(), None);
}
#[test]
fn the_oldest_page_is_dropped_at_capacity() {
    let mut service = service(1, Duration::from_secs(3600));
    let first = manifest(&mut service, 1);
    let second = manifest(&mut service, 2);
    assert!(service.setup_page(1, &first, Time::ZERO).is_none());
    assert!(service.setup_page(1, &second, Time::ZERO).is_some());
}
#[test]
fn an_install_can_evict_a_manifests_state_while_its_page_lives() {
    let mut service = service(1, Duration::from_secs(3600));
    let state = manifest(&mut service, 1);
    row(&mut service, Item::AppSlug, b"app-fixture");
    start(&mut service, 2, host::Plan::Install { forge: 1 });
    assert!(let_done(terminal(&mut service)));
    assert!(service.setup_page(1, &state, Time::ZERO).is_some());
    // The slug above only made an Install-purpose state; no App remains in this host-state fixture.
    service.engine.as_mut().expect("engine").forge_records.rows = crate::forge::records::Loader::new(
        &fixtures::engine().limits,
        &service.engine.as_ref().expect("engine").workers.forges,
    )
    .rows;
    start(&mut service, 3, host::Plan::Convert { forge: 1, code: code(), state });
    let root::SetupEnd::Stopped { forge, reason } = terminal(&mut service) else { panic!("evicted state refused") };
    assert_eq!(forge, 1);
    assert!(reason.ends_with(b"Unknown"));
    assert!(service.engine.as_ref().expect("engine").setup.io.is_empty(), "evicted state sends no conversion");
}
fn let_done(end: root::SetupEnd) -> bool {
    match end {
        root::SetupEnd::Done { .. } => true,
        root::SetupEnd::Stopped { .. } => false,
    }
}
fn code() -> Secret {
    Secret::new(Box::from(skein_lib::Decimal::of(Rng::new(805).next_u64()).as_bytes()))
}

pub(super) struct Face {
    owner: Option<Token>,
    hold_done: bool,
    done: Option<host::Event>,
    pub(super) wire: skein_world::stream::Wire,
}
impl Face {
    pub(super) fn new() -> Self {
        Self { owner: None, hold_done: false, done: None, wire: skein_world::stream::Wire::new(7) }
    }
    pub(super) fn turn(&mut self, service: &mut crate::Service) {
        for _ in 0_u32..service.engine.as_mut().expect("engine").setup.io.len() {
            match service.engine.as_mut().expect("engine").setup.io.pop().unwrap() {
                skein_io::Request::Connect { owner, .. } => {
                    self.owner = Some(owner);
                    self.wire = skein_world::stream::Wire::new(7);
                    service
                        .engine
                        .as_mut()
                        .expect("engine")
                        .setup
                        .up(skein_io::Event::Connecting { owner, socket: Token::new(123) });
                    service.engine.as_mut().expect("engine").setup.up(skein_io::Event::Connected { owner });
                }
                skein_io::Request::Stream { down, .. } => self.wire.take(down),
                skein_io::Request::Close { .. } | skein_io::Request::Abort { .. } => {
                    service
                        .engine
                        .as_mut()
                        .expect("engine")
                        .setup
                        .up(skein_io::Event::Closed { owner: self.owner.take().unwrap() });
                }
                skein_io::Request::Listen { .. }
                | skein_io::Request::Bind { .. }
                | skein_io::Request::Reject { .. }
                | skein_io::Request::Output { .. }
                | skein_io::Request::Spawn { .. }
                | skein_io::Request::Signal { .. }
                | skein_io::Request::Usage { .. }
                | skein_io::Request::Window { .. } => panic!("setup socket request"),
            }
        }
        if let Some(owner) = self.owner
            && let Some(up) = self.wire.answer()
        {
            service.engine.as_mut().expect("engine").setup.up(skein_io::Event::Stream { owner, up });
        }
        if self.hold_done {
            let engine = service.engine.as_mut().unwrap();
            for _ in 0..engine.setup.above.len() {
                let event = engine.setup.above.pop().unwrap();
                match event {
                    done @ host::Event::Done { .. } => self.done = Some(done),
                    other @ (host::Event::Need { .. } | host::Event::Stop { .. } | host::Event::Closed) => {
                        engine.setup.above.push(other);
                    }
                }
            }
        }
        advance(service.engine.as_mut().expect("engine"));
    }
    fn reply(&mut self, service: &mut crate::Service, body: &[u8]) {
        self.reply_status(service, b"200 OK", body);
    }
    fn reply_status(&mut self, service: &mut crate::Service, status: &[u8], body: &[u8]) {
        let length = skein_lib::Decimal::of(u64::try_from(body.len()).unwrap());
        let head = b"HTTP/1.1 ";
        let content = b"\r\nContent-Length: ";
        let mut writer = skein_lib::Writer::new(
            head.len()
                .checked_add(status.len())
                .unwrap()
                .checked_add(content.len())
                .unwrap()
                .checked_add(length.as_bytes().len())
                .unwrap()
                .checked_add(4)
                .unwrap()
                .checked_add(body.len())
                .unwrap(),
        );
        writer.put(head).unwrap();
        writer.put(status).unwrap();
        writer.put(content).unwrap();
        writer.put(length.as_bytes()).unwrap();
        writer.put(b"\r\n\r\n").unwrap();
        writer.put(body).unwrap();
        self.wire.write(&writer.finish());
        for _ in 0_u32..1024 {
            self.turn(service);
        }
    }
    pub(super) fn sent(&mut self, service: &mut crate::Service) {
        for _ in 0_u32..1024 {
            self.turn(service);
        }
        assert!(contains(&self.wire.received, b"\r\n\r\n"), "request sent");
    }
}
fn writing() -> (crate::Service, Face) {
    let mut service = service(8, Duration::from_secs(3600));
    let state = manifest(&mut service, 1);
    start(&mut service, 2, host::Plan::Convert { forge: 1, code: code(), state });
    let face = conversion_replies(&mut service);
    (service, face)
}
fn conversion_replies(service: &mut crate::Service) -> Face {
    let mut face = Face::new();
    face.sent(service);
    let der = skein_fake_forge_github::keys::KEYS[0];
    let encoded =
        skein_lib::base64::encoded_len(skein_lib::base64::Alphabet::Standard, u32::try_from(der.len()).unwrap())
            .unwrap();
    let mut base64 = skein_lib::Writer::new(usize::try_from(encoded).unwrap());
    skein_lib::base64::encode(skein_lib::base64::Alphabet::Standard, der, &mut base64).unwrap();
    let prefix = br#"{"id":7,"slug":"app-fixture","owner":{"id":1,"login":"owner-fixture"},"pem":"-----BEGIN RSA PRIVATE KEY-----\n"#;
    let suffix = br#"\n-----END RSA PRIVATE KEY-----\n","client_id":"fixture","client_secret":"unused","webhook_secret":"unused"}"#;
    let mut body = skein_lib::Writer::new(
        prefix.len().checked_add(usize::try_from(encoded).unwrap()).unwrap().checked_add(suffix.len()).unwrap(),
    );
    body.put(prefix).unwrap();
    body.put(&base64.finish()).unwrap();
    body.put(suffix).unwrap();
    face.reply(service, &body.finish());
    face.sent(service);
    face.reply(service, br#"{"id":7,"slug":"app-fixture"}"#);
    face.sent(service);
    face.reply(service, br#"{"id":9,"login":"app-fixture[bot]"}"#);
    if service.engine.as_mut().expect("engine").setup.writes.is_empty() {
        if let Some(root::Event::Setup { end, .. }) = service.engine.as_mut().expect("engine").events.pop() {
            panic!("conversion terminal: {end:?}");
        }
        panic!(
            "conversion pending: owner {}, incoming {}, outgoing {}",
            face.owner.is_some(),
            service.engine.as_mut().expect("engine").setup.above.len(),
            service.engine.as_mut().expect("engine").setup.io.len()
        );
    }
    face
}
#[test]
fn written_is_answered_only_after_kept_and_the_routing_rows_follow_each_keep() {
    let (mut service, mut face) = writing();
    for expected in [Item::AppId, Item::AppSlug, Item::AppKey, Item::User] {
        assert!(service.engine.as_ref().expect("engine").setup.answers.is_empty(), "Written must wait for Kept");
        assert!(
            (match service.engine.as_ref().expect("engine").forge_records.rows.forge(1) {
                Some(rows) => rows.get(expected).ok(),
                None => None,
            })
            .is_none(),
            "unkept row is invisible"
        );
        let owner = *service.engine.as_mut().expect("engine").setup.writes.first().unwrap().0;
        let write = service.engine.as_mut().expect("engine").setup.writes.get(&owner).unwrap();
        assert_eq!(write.number, records::number(1, expected));
        assert_eq!(write.value.expose().first(), Some(&records::VERSION));
        answer(service.engine.as_mut().expect("engine"), StoreEvent::Kept { owner });
        assert_eq!(
            service.engine.as_mut().expect("engine").setup.answers.pop(),
            Some((Token::new(2), host::Answer::Written))
        );
        service.engine.as_mut().expect("engine").forge_records.rows.forge(1).unwrap().get(expected).unwrap();
        face.turn(&mut service);
    }
    advance(service.engine.as_mut().expect("engine"));
    let owner = *service.engine.as_mut().expect("engine").setup.writes.first().unwrap().0;
    assert_eq!(
        service.engine.as_ref().expect("engine").setup.writes.get(&owner).unwrap().number,
        records::number(1, Item::Origin)
    );
    answer(service.engine.as_mut().expect("engine"), StoreEvent::Kept { owner });
    assert!(let_done(terminal(&mut service)));
}
#[test]
fn a_failed_keep_stops_the_engine_and_never_answers_written() {
    let (mut service, _face) = writing();
    let owner = *service.engine.as_mut().expect("engine").setup.writes.first().unwrap().0;
    let Some(StoreEvent::Kept { owner: kept }) = service.engine.as_mut().expect("engine").store.above.pop() else {
        panic!("pending keep terminal")
    };
    assert_eq!(kept, owner);
    service.engine.as_mut().expect("engine").store.above.push(StoreEvent::Corrupt { owner });
    crate::engine::iterate(service.engine.as_mut().expect("engine"), Time::ZERO, Wall::EPOCH);
    assert!(service.end().is_some());
    assert!(service.engine.as_ref().expect("engine").setup.answers.is_empty());
}
#[test]
fn draining_closes_the_host_and_waits_for_closed() {
    let mut service = service(8, Duration::from_secs(3600));
    let state = manifest(&mut service, 1);
    start(&mut service, 2, host::Plan::Convert { forge: 1, code: code(), state });
    let mut face = Face::new();
    face.sent(&mut service);
    crate::stop::shutdown(service.engine.as_mut().expect("engine"));
    crate::stop::drained(service.engine.as_mut().expect("engine"));
    advance(service.engine.as_mut().expect("engine"));
    assert!(!service.engine.as_ref().expect("engine").setup.closed, "Closed waits for the live socket");
    face.turn(&mut service);
    assert!(service.engine.as_ref().expect("engine").setup.closed);
    assert!(service.engine.as_ref().expect("engine").setup.pages.is_empty());
    assert_eq!(service.engine.as_ref().expect("engine").setup.next_deadline(), None);
}
fn tls_forges() -> (crate::settings::Engine, List<crate::ForgeRuntime>) {
    // Skein's fixed TLS-world root, carried here without another test dependency.
    let root = skein_lib::base64::decode(
        skein_lib::base64::Alphabet::Standard,
        concat!(
            "MIIBmDCCAT+gAwIBAgIUHvatnWfGKnu6ssb8pgtoqmWxTcQwCgYIKoZIzj0EAwIwGjEYMBYGA1UEAwwPc2tlaW4gdGVzdCByb290",
            "MB4XDTI2MDEwMTAwMDAwMFoXDTM2MDEwMTAwMDAwMFowGjEYMBYGA1UEAwwPc2tlaW4gdGVzdCByb290MFkwEwYHKoZIzj0CAQYI",
            "KoZIzj0DAQcDQgAElnuG+BWgtzb++8N0S1weL+2KuE3Fb9nx992dyCDQ6GJdAwludLQPNm9HNipn55h9kfFjKtdy/de0tsxuavfE",
            "UqNjMGEwHQYDVR0OBBYEFPmq8/JFoN7ZJ8taLdr3/hphqvWjMB8GA1UdIwQYMBaAFPmq8/JFoN7ZJ8taLdr3/hphqvWjMA8GA1Ud",
            "EwEB/wQFMAMBAf8wDgYDVR0PAQH/BAQDAgEGMAoGCCqGSM49BAMCA0cAMEQCIEtpmEfeTnDuXGC/OGm3Q7fM97DmGIwA3AUsfwTn",
            "AaLNAiAdXjbm1OYLV5vW4t68UIk4Kzxdk5qEue0bPNUZ9oISMw==",
        )
        .as_bytes(),
        412,
    )
    .unwrap();
    let mut config = fixtures::engine();
    config.forges =
        Box::new([fixtures::forges().into_iter().next().unwrap(), fixtures::forges().into_iter().next().unwrap()]);
    let mut runtime = List::with_capacity(2);
    for (index, (number, name, address)) in
        [(1, "first.test", [192, 0, 2, 7]), (2, "second.test", [192, 0, 2, 8])].into_iter().enumerate()
    {
        config.forges[index].number = number;
        config.forges[index].kind = crate::settings::Kind::GitHub;
        config.forges[index].api_token = None;
        config.forges[index].transport =
            crate::settings::Transport::Tls { server_name: name.as_bytes().into(), trust: None };
        let (trust, _) = skein_tls::Config::from_der(core::slice::from_ref(&root), &[name.as_bytes()]).unwrap();
        runtime
            .push(crate::ForgeRuntime {
                number,
                address: (address, 443).into(),
                transport: skein_forge_connection::settings::Transport::Tls {
                    server_name: skein_tls::Name::new(name).unwrap(),
                    trust,
                },
                trust_roots: 1,
                trust_root_bytes: 412,
            })
            .unwrap();
    }
    (config, runtime)
}
#[test]
#[expect(
    clippy::disallowed_macros,
    reason = "compare each prepared public trust configuration without exposing credentials"
)]
fn each_github_tls_entry_keeps_its_prepared_name_trust_and_address() {
    let (config, runtime) = tls_forges();
    let (mut endpoints, _) = crate::forge::prepare(&config, runtime).unwrap();
    let setup = prepare(&config, &mut endpoints, &fixtures::seeds().crypto, 2, false).unwrap();
    assert_eq!(setup.served.len(), 2);
    let (settings, _) = setup.pending.unwrap();
    assert_eq!(settings.forges.len(), 2);
    for endpoint in &endpoints.forges {
        let mut entry = None;
        for candidate in &settings.forges {
            if candidate.number() == endpoint.number {
                entry = Some(candidate);
                break;
            }
        }
        let entry = entry.unwrap();
        assert_eq!(entry.address, endpoint.address);
        let host::Transport::Tls { server_name, trust } = &entry.transport else { panic!("TLS entry") };
        let skein_forge_connection::settings::Transport::Tls { server_name: name, trust: prepared } =
            &endpoint.transport
        else {
            panic!("prepared TLS")
        };
        assert_eq!(server_name, name);
        assert_eq!(alloc::format!("{trust:?}"), alloc::format!("{prepared:?}"));
    }
}
#[test]
fn tls_selects_a_larger_host_price_than_plaintext() {
    let (mut config, mut runtime) = tls_forges();
    // Both transports use the ciphertext room TLS requires as their common io ceiling.
    config.jig.limits.io.output = skein_tls::client::largest_room(&config.limits.forge_component.tls);
    for index in 0..runtime.len() {
        runtime.get_mut(index).unwrap().address.set_ip([127, 0, 0, 1].into());
    }
    let (mut endpoints, _) = crate::forge::prepare(&config, runtime).unwrap();
    let setup = prepare(&config, &mut endpoints, &fixtures::seeds().crypto, 2, false).unwrap();
    let (mut settings, _) = setup.pending.unwrap();
    let tls = host::worst_case(&setup.env.limits, &settings).unwrap();
    for index in 0..settings.forges.len() {
        let entry = settings.forges.get_mut(index).unwrap();
        let host::Transport::Tls { server_name, .. } = &entry.transport else { panic!("TLS entry") };
        let skein_tls::Server::Dns(name) = server_name.server() else { panic!("fixture DNS name") };
        entry.transport = host::Transport::Plaintext { name: Some(name.as_bytes().into()) };
    }
    assert!(tls > host::worst_case(&setup.env.limits, &settings).unwrap());
}
#[test]
fn admitted_tls_starts_after_rows_and_serves_a_local_manifest() {
    let required = accounted(8, 8, true);
    let (mut config, runtime) = configured(8, 8, true);
    config.jig.memory = required;
    let mut service = crate::Service::engine(config, &fixtures::seeds(), None, runtime).unwrap();
    assert_eq!(service.worst_case(), required);
    assert!(service.engine.as_ref().expect("engine").setup.component.is_none(), "no host before the rows load");
    let (settings, _) = service.engine.as_ref().expect("engine").setup.pending.as_ref().unwrap();
    assert_eq!(settings.forges.len(), 2);
    for entry in &settings.forges {
        let host::Transport::Tls { .. } = &entry.transport else { panic!("admitted TLS") };
        assert!(service.engine.as_ref().expect("engine").setup.served.contains_key(&entry.number()));
    }
    service.engine.as_mut().expect("engine").store = fixtures::records_store();
    for _ in 0_u32..8 {
        crate::forge::records::advance(service.engine.as_mut().expect("engine"));
    }
    advance(service.engine.as_mut().expect("engine"));
    assert!(service.engine.as_ref().expect("engine").forge_records.done());
    assert!(service.engine.as_ref().expect("engine").setup.component.is_some());
    for _ in 0..service.engine.as_mut().expect("engine").notes.len() {
        let _startup_note = service.engine.as_mut().expect("engine").notes.pop();
    }
    let state = manifest(&mut service, 1);
    assert!(service.setup_page(1, &state, Time::ZERO).is_some());
    assert!(service.end().is_none());
    assert!(service.engine.as_ref().expect("engine").setup.io.is_empty(), "manifest is local");
}
#[test]
fn the_host_names_the_metadata_limits_and_rejects_bad_start_limits() {
    let mut service = service(8, Duration::from_secs(3600));
    let setup = &mut service.engine.as_mut().expect("engine").setup;
    assert_eq!(setup.env.limits.inner, 32);
    let mut found = jig_protocol_config::Problems::empty();
    settings::problems(host::Unusable::GitHubMetadata { inner: 1, string_bytes: 1 }, &mut found);
    for (index, name) in [b"forge_component.inner".as_slice(), b"forge_component.string_bytes"].into_iter().enumerate()
    {
        assert_eq!(found.problems.get(u32::try_from(index).unwrap()).unwrap().path, settings::limit_path(name));
    }
}

fn contains(bytes: &[u8], needle: &[u8]) -> bool {
    for part in bytes.windows(needle.len()) {
        if part == needle {
            return true;
        }
    }
    false
}
#[test]
fn setup_memory_includes_the_whole_host_and_its_owner() {
    let mut config = fixtures::engine();
    config.forges[0].kind = crate::settings::Kind::GitHub;
    config.identities[0].token = Secret::new(Box::new([]));
    config.identities[0].login = Box::new([]);
    config.identities[0].author.name = Box::new([]);
    config.identities[0].author.email = Box::new([]);
    config.forges[0].api_token = None;
    let (mut endpoints, _) = crate::forge::prepare(&config, fixtures::forge_runtime()).unwrap();
    let setup = prepare(&config, &mut endpoints, &fixtures::seeds().crypto, 2, false).unwrap();
    let (settings, _) = setup.pending.unwrap();
    assert_eq!(settings.user_agent.as_ref(), crate::forge::USER_AGENT.as_bytes());
    let given = settings.forges.get(0).unwrap().github().unwrap();
    let machines = skein_forge_setup::github::worst_case(given, 8, 8).unwrap();
    let rows = crate::forge::records::Rows::worst_case(
        u32::try_from(config.forges.len()).unwrap(),
        config.limits.records.value_bytes,
    )
    .unwrap();
    let pages = 8_u64.checked_mul(u64::from(given.limits.page_bytes)).unwrap();
    // Create enlarges the shared plan/need carriers: eight runs add 384 B.
    assert_eq!((machines, rows, pages), (631_128, 1_825_634, 131_072));
    let whole = worst_case(&config, &endpoints, 2).unwrap();
    assert!(whole > host::worst_case(&setup.env.limits, &settings).unwrap());
    assert!(accounted(8, 8, false) > accounted(1, 1, false), "runs/states change the admitted shell price");
}
#[test]
fn every_host_unusable_arm_has_an_operator_path() {
    use host::Stacked::{HttpOverTls, TlsOverIo};
    use host::{Stacked, Unusable};
    for why in [
        Unusable::Zero,
        Unusable::TokenBytes { token_bytes: 1 },
        Unusable::Documents(skein_forge::forgejo::limits::Problem::Depth),
        Unusable::GitHubDocuments(skein_forge::github::limits::Problem::Depth),
        Unusable::GitHubMetadata { inner: 1, string_bytes: 1 },
        Unusable::Stack { stacked: Stacked::HttpOverIo, demand: 2, cap: 1 },
        Unusable::Stack { stacked: HttpOverTls, demand: 2, cap: 1 },
        Unusable::Stack { stacked: TlsOverIo, demand: 2, cap: 1 },
        Unusable::Stack { stacked: Stacked::JsonOverHttp, demand: 2, cap: 1 },
        Unusable::Forges { forges: 1 },
        Unusable::Duplicate { forge: 1 },
        Unusable::Address { forge: 1 },
        Unusable::PlaintextName { forge: 1 },
        Unusable::Request { forge: 1 },
        Unusable::Given {
            forge: 1,
            refused: skein_forge_setup::given::Refused {
                place: skein_forge_setup::given::Place::Deadline,
                other: None,
            },
        },
        Unusable::GitHubGiven {
            forge: 1,
            refused: skein_forge_setup::github::given::Refused {
                place: skein_forge_setup::github::given::Place::Backoff,
                other: Some(skein_forge_setup::github::given::Place::BackoffMax),
            },
        },
        Unusable::Reachable { forge: 1, reachable: 2, inner: 1 },
    ] {
        let mut found = jig_protocol_config::Problems::empty();
        settings::problems(why, &mut found);
        assert!(!found.is_empty());
        let Unusable::Stack { stacked, .. } = why else { continue };
        let expected: &[&[u8]] = match stacked {
            HttpOverTls => &[b"forge_component.chunk", b"forge_component.tls.read", b"forge_component.tls.send"],
            TlsOverIo => &[b"forge_component.tls.read", b"forge_component.tls.send", b"io.intake", b"io.output"],
            Stacked::HttpOverIo | Stacked::JsonOverHttp => continue,
        };
        assert_eq!(found.problems.len(), u32::try_from(expected.len()).unwrap());
        for (problem, name) in found.problems.iter().zip(expected) {
            assert_eq!(problem.path, settings::limit_path(name));
        }
    }
}
#[test]
fn invalid_host_limits_stop_after_load_and_setup_overrides_are_checked() {
    let mut service = service(8, Duration::from_secs(3600));
    let mut config = fixtures::engine();
    config.forges[0].kind = crate::settings::Kind::GitHub;
    config.identities[0].token = Secret::new(Box::new([]));
    config.identities[0].login = Box::new([]);
    config.identities[0].author.name = Box::new([]);
    config.identities[0].author.email = Box::new([]);
    config.forges[0].api_token = None;
    config.limits.forge_component.inner = 13;
    config.limits.picker.page = 1;
    let (mut endpoints, _) = crate::forge::prepare(&fixtures::engine(), fixtures::forge_runtime()).unwrap();
    service.engine.as_mut().expect("engine").setup =
        prepare(&config, &mut endpoints, &fixtures::seeds().crypto, 2, false).unwrap();
    advance(service.engine.as_mut().expect("engine"));
    assert!(service.end().is_some());
    let mut own = crate::limits::profile(crate::limits::Profile::Small);
    for name in [b"setup.states".as_slice(), b"setup.runs", b"setup.state_life_ms", b"setup.orgs"] {
        assert_eq!(crate::limits::apply(&mut own, name, 0), Err(crate::limits::Refused::Range));
    }
    assert_eq!(crate::limits::apply(&mut own, b"setup.runs", 3), Ok(()));
    assert_eq!(own.setup.runs, 3);
}

#[test]
fn stopped_connection_text_has_a_fixed_bound() {
    use skein_forge_connection::{StartRead, Stopped, Value};
    let mut reasons = List::with_capacity(13);
    for value in [Value::Credential, Value::Token, Value::Key, Value::User, Value::Secret] {
        reasons.push(Stopped::Invalid { value }).unwrap();
    }
    for read in [StartRead::User, StartRead::ApiSettings, StartRead::Installations, StartRead::Mint, StartRead::Viewer]
    {
        reasons.push(Stopped::Refused { read, status: u16::MAX }).unwrap();
    }
    reasons.push(Stopped::TokenRefused { installation: u64::MAX, status: u16::MAX }).unwrap();
    reasons.push(Stopped::OtherUser { user: u64::MAX }).unwrap();
    reasons.push(Stopped::KeyRefused).unwrap();
    for why in &reasons {
        assert!(
            u64::try_from(lifecycle::stopped_text(*why).len()).unwrap()
                <= temper_engine_domain_forge::STOPPED_TEXT_BYTES
        );
    }
}
fn configured(runs: u32, states: u32, tls: bool) -> (crate::settings::Engine, List<crate::ForgeRuntime>) {
    let (mut config, runtime) = if tls { tls_forges() } else { (fixtures::engine(), fixtures::forge_runtime()) };
    config.forges[0].kind = crate::settings::Kind::GitHub;
    config.identities[0].token = Secret::new(Box::new([]));
    config.identities[0].login = Box::new([]);
    config.identities[0].author.name = Box::new([]);
    config.identities[0].author.email = Box::new([]);
    config.forges[0].api_token = None;
    config.limits.setup.runs = runs;
    config.limits.setup.states = states;
    fixtures::fill_files(&mut config);
    (config, runtime)
}
#[expect(clippy::arithmetic_side_effects, reason = "the measured nonzero bound permits one byte less")]
fn accounted(runs: u32, states: u32, tls: bool) -> u64 {
    let (config, runtime) = configured(runs, states, tls);
    let required = crate::Service::engine(config, &fixtures::seeds(), None, runtime).unwrap().worst_case();
    for short in [false, true] {
        let (mut config, runtime) = configured(runs, states, tls);
        config.jig.memory = required - u64::from(short);
        let started = crate::Service::engine(config, &fixtures::seeds(), None, runtime);
        if short {
            assert_eq!(
                started.err().unwrap().problems.get(0).unwrap().what,
                jig_protocol_config::What::Memory { over: 1 }
            );
        } else {
            assert_eq!(started.unwrap().worst_case(), required);
        }
    }
    required
}

#[path = "setup_reachable.rs"]
mod reachable_tests;

#[path = "setup_lifecycle.rs"]
mod lifecycle_tests;

#[path = "setup_adoption.rs"]
mod adoption_tests;

#[test]
fn an_unresolved_forge_keeps_its_setup_entry_but_restored_and_asked_connects_never_dial() {
    let mut config = fixtures::engine();
    config.forges[0].kind = crate::settings::Kind::GitHub;
    config.forges[0].api = jig_protocol_config::Address::Host { name: b"forge.example".as_slice().into(), port: 443 };
    config.forges[0].api_token = None;
    let mut other = fixtures::forges().into_iter().next().unwrap();
    other.number = 2;
    let first = config.forges.into_iter().next().unwrap();
    config.forges = Box::new([first, other]);
    fixtures::fill_files(&mut config);
    let mut runtime = fixtures::forge_runtime();
    runtime.get_mut(0).unwrap().address = crate::forge::HELD_ADDRESS;
    let mut endpoints = List::with_capacity(2);
    endpoints.push(runtime.into_boxed().into_iter().next().unwrap()).unwrap();
    let mut other = fixtures::forge_runtime().into_boxed().into_iter().next().unwrap();
    other.number = 2;
    endpoints.push(other).unwrap();
    let mut service = crate::Service::engine(config, &fixtures::seeds(), None, endpoints).unwrap();
    let setup = &service.engine.as_ref().unwrap().setup;
    assert!(setup.served.contains_key(&1), "the held forge still has a setup entry");
    let (settings, _) = setup.pending.as_ref().unwrap();
    assert_eq!(settings.forges.len(), 2, "both the held GitHub and connected Forgejo serve picker reads");
    assert_eq!(settings.forges.get(0).unwrap().address, skein_io::Addr::Ip(crate::forge::HELD_ADDRESS));
    lifecycle_tests::app(&mut service);
    let engine = service.engine.as_mut().unwrap();
    let expected = records::origin(skein_forge_connection::settings::Kind::GitHub, &engine.workers.forges[0].api);
    assert!(
        records::access(
            skein_forge_connection::settings::Kind::GitHub,
            &expected,
            engine.forge_records.rows.forge(1).unwrap(),
            &engine.forge_records.bounds,
            false
        )
        .is_ok(),
        "restorable access"
    );
    engine.forge_records.loaded(Box::new([]), None);
    for _ in 0_u32..8 {
        crate::forge::records::advance(engine);
    }
    assert_eq!(
        engine.notes.pop(),
        Some(crate::Note::ForgeNotConnected {
            number: 1,
            why: crate::NotConnected::Unreachable { host: b"forge.example".as_slice().into() }
        })
    );
    assert!(engine.forge.starts.is_empty());
    engine.forge.iterate(Time::ZERO, Wall::EPOCH);
    let mut other_connects = 0_u32;
    for request in &engine.forge.io {
        if let skein_io::Request::Connect { addr, .. } = request {
            assert_eq!(*addr, skein_io::Addr::Ip(([127, 0, 0, 1], 3000).into()));
            other_connects += 1;
        }
    }
    assert_eq!(other_connects, 1, "the resolved forge connects normally");
    advance(engine);
    for (run, plan) in
        [(1, host::Plan::Manifest { forge: 1, target: Target::Personal }), (2, host::Plan::Install { forge: 1 })]
    {
        start(&mut service, run, plan);
        let root::SetupEnd::Stopped { forge, reason } = terminal(&mut service) else { panic!("held setup refused") };
        assert_eq!(forge, 1);
        assert_eq!(reason.as_ref(), b"unreachable: forge.example");
    }
    let engine = service.engine.as_ref().unwrap();
    assert!(engine.forge.starts.is_empty());
    assert!(engine.setup.io.is_empty(), "no held setup request reaches a socket");
    assert!(engine.setup.runs.is_empty(), "the host never receives a held plan");
    assert_eq!(engine.domain.forge_connection(1), temper_engine_domain_forge::Connection::NotConnected);
}

#[path = "git_tokens.rs"]
mod git_tokens;

fn local_config(records: bool, fake: bool) -> crate::settings::Engine {
    let mut config = fixtures::engine();
    config.forges[0].api = jig_protocol_config::Address::Addr(skein_io::Addr::Ip(([127, 0, 0, 1], 9).into()));
    config.setup = Box::new([fixtures::owned_setup(1)]);
    config.setup[0].entry.asked = false;
    if fake {
        config.setup[0].entry.mode = temper_forge_setup::setup::Mode::Fake(temper_forge_setup::setup::Fake {
            listen: ([127, 0, 0, 1], 9).into(),
            git: b"/data/git".as_slice().into(),
            owner: b"demo".as_slice().into(),
            repositories: Box::new([]),
        });
    }
    if records {
        config.records_only = Box::new([1]);
        config.forges[0].api_token = None;
        config.forges[0].user = 0;
        config.identities[0].token = Secret::new(Box::new([]));
        config.identities[0].login = if fake { Box::new([]) } else { b"git-user".as_slice().into() };
    }
    config.limits.forge.client.repositories = 3;
    config.limits.forge.output = 96;
    fixtures::fill_files(&mut config);
    config
}
fn kept_record(store: &mut crate::store::Store, forge: u16, item: Item, bytes: &[u8]) {
    let owner = Token::new(901);
    assert!(
        store
            .secrets(
                owner,
                jig_protocol_store::Secrets::Keep {
                    owner: jig_protocol_store::SecretOwner::Application,
                    number: records::number(forge, item),
                    value: Secret::new(records::encode(bytes).into_boxed_slice()),
                }
            )
            .is_none()
    );
    let Some(StoreEvent::Kept { owner: found }) = store.above.pop() else {
        panic!("durable seed");
    };
    assert_eq!(found, owner);
}
fn local_store(token: bool) -> crate::store::Store {
    use temper_engine_domain_forge::Stored;
    // Persist the real first-start settings before constructing a restart fixture.
    let mut bootstrap = fixtures::ready();
    let mut header = bootstrap.engine.as_ref().unwrap().domain.deployment();
    header.commits = header.commits.checked_add(1).unwrap();
    let mut store = bootstrap.engine.take().unwrap().store;
    if token {
        kept_record(&mut store, 1, Item::AdminToken, b"test-admin");
    }
    for (item, value) in
        [(Item::ApiToken, b"test-api".as_slice()), (Item::GitToken, b"test-git"), (Item::User, &42_u64.to_le_bytes())]
    {
        kept_record(&mut store, 1, item, value);
    }
    let row = local_repository();
    assert!(store.down(root::StoreRequest::Commit {
        number: header.commits,
        writes: Box::new([
            root::Write::Save(root::Record::Core(jig_domain_core::Record::Core(
                jig_domain_core::CoreRecord::Deployment(header),
            ))),
            root::Write::Save(root::Record::Forge { row: Box::new(Stored::Repository(row)) }),
        ]),
    }));
    let Some(StoreEvent::Committed { number, .. }) = store.above.pop() else {
        panic!("durable repository");
    };
    assert_eq!(number, header.commits);
    store
}
fn local_repository() -> domain::Repository {
    use temper_engine_domain_forge::{Kinds, Protection, Repository, Role};
    Repository {
        project: 1,
        home: true,
        provider: skein_forge::api::Repository { forge: 1, repository: 41 },
        host: b"forge.example".as_slice().into(),
        owner: b"demo".as_slice().into(),
        name: b"widget".as_slice().into(),
        prefix: b"temper/".as_slice().into(),
        role: Role::Owned,
        ci: true,
        checks: Box::new([]),
        kinds: Kinds {
            read: true,
            push: true,
            open: true,
            land: true,
            review: true,
            status: true,
            comment: true,
            issue: true,
            branch: true,
        },
        protection: Protection::Absent,
        settings: skein_forge::api::Settings {
            default_branch: b"main".as_slice().into(),
            merge: true,
            squash: true,
            rebase: false,
        },
    }
}
fn local_service(config: crate::settings::Engine, store: crate::store::Store) -> crate::Service {
    let mut service = crate::Service::engine(config, &fixtures::seeds(), None, fixtures::forge_runtime()).unwrap();
    service.engine.as_mut().unwrap().store = store;
    service
}
fn restore_local(service: &mut crate::Service) {
    // The generic kernel refuses unhandled network connects. Hold local work
    // until restore completes, then give its real setup streams to Face.
    let pending = match service.engine.as_mut().unwrap().setup.lifecycle.local_mut(1) {
        Some(local) => {
            let pending = local.pending;
            local.pending = false;
            Some(pending)
        }
        None => None,
    };
    for _ in 0_u32..4096 {
        fixtures::turn(service);
        let engine = service.engine.as_mut().unwrap();
        if engine.domain.ready() {
            if let Some(pending) = pending {
                engine.setup.lifecycle.local_mut(1).unwrap().pending = pending;
            }
            crate::engine::iterate(engine, Time::from_nanos(1), Wall::from_nanos(1));
            advance(engine);
            return;
        }
    }
    let engine = service.engine.as_ref().unwrap();
    panic!(
        "root restore completes: end={:?}, step={:?}, settings={:?}, header={:?}",
        service.end(),
        engine.domain.restart_failure(),
        engine.domain.settings_failure(),
        engine.domain.deployment(),
    );
}
fn local_sent(service: &mut crate::Service, face: &mut Face, path: &[u8]) {
    face.sent(service);
    assert!(contains(&face.wire.received, path), "wanted path {path:?}");
}
fn reconcile_to_repository(service: &mut crate::Service, face: &mut Face) {
    local_sent(service, face, b"GET /api/v1/user ");
    assert!(contains(&face.wire.received, b"test-admin"));
    face.reply(service, br#"{"id":1,"login":"temper-admin","is_admin":true}"#);
    local_sent(service, face, b"GET /api/v1/users/temper ");
    face.reply(service, br#"{"id":42,"login":"temper","full_name":"temper:bot","is_admin":false}"#);
    for token in [b"test-api".as_slice(), b"test-git"] {
        local_sent(service, face, b"GET /api/v1/user ");
        assert!(contains(&face.wire.received, token));
        face.reply(service, br#"{"id":42,"login":"temper"}"#);
    }
    local_sent(service, face, b"GET /api/v1/repositories/41 ");
    face.reply(service, br#"{"id":41,"owner":{"id":1,"login":"demo"},"name":"widget","object_format_name":"sha1","default_branch":"main","default_merge_style":"merge","allow_merge_commits":true,"allow_squash_merge":true,"allow_rebase":true,"html_url":"http://127.0.0.1:9/demo/widget"}"#);
    local_sent(service, face, b"/collaborators/temper/permission ");
}
#[test]
fn kept_local_records_wait_for_restore_done_and_origin_before_connecting_as_the_recorded_bot() {
    for fake in [false, true] {
        let mut service = local_service(local_config(true, fake), local_store(true));
        let engine = service.engine.as_mut().unwrap();
        for _ in 0_u32..8 {
            crate::forge::records::advance(engine);
        }
        advance(engine);
        assert!(engine.setup.runs.is_empty(), "no Reconcile before root restore");
        assert!(engine.forge.starts.is_empty());
        assert!(engine.domain.adopted_ids(1).is_empty());
        restore_local(&mut service);
        assert_eq!(service.engine.as_ref().unwrap().domain.adopted_ids(1).as_ref(), &[41]);
        let engine = service.engine.as_mut().unwrap();
        let before = (engine.setup.io.len(), engine.setup.runs.len(), engine.setup.writes.len());
        crate::engine::iterate(engine, Time::from_nanos(1), Wall::from_nanos(1));
        assert_eq!(
            (engine.setup.io.len(), engine.setup.runs.len(), engine.setup.writes.len()),
            before,
            "a second restore iterate changes no local setup work"
        );
        // A completed walk is never repeated, including for an already stopped local start.
        engine.setup.lifecycle.local_mut(1).unwrap().restored = false;
        lifecycle::restored(engine);
        assert!(!engine.setup.lifecycle.local(1).unwrap().restored);
        engine.setup.lifecycle.local_mut(1).unwrap().restored = true;
        let mut face = Face::new();
        reconcile_to_repository(&mut service, &mut face);
        assert!(service.engine.as_ref().unwrap().forge.starts.is_empty(), "held Reconcile has no access");
        face.hold_done = true;
        face.reply(&mut service, br#"{"permission":"write","user":{"id":42,"login":"temper"}}"#);
        assert!(face.done.is_some(), "the actual host completed Reconcile");
        assert!(service.engine.as_ref().unwrap().setup.writes.is_empty(), "no Origin before Done");
        assert!(service.engine.as_ref().unwrap().forge.starts.is_empty());
        service.engine.as_mut().unwrap().setup.above.push(face.done.take().unwrap());
        advance(service.engine.as_mut().unwrap());
        let engine = service.engine.as_mut().unwrap();
        assert_eq!(engine.setup.writes.len(), 1);
        let write = engine.setup.writes.first().unwrap().1;
        assert_eq!(write.number, records::number(1, Item::Origin), "never writes AdminToken");
        assert!(engine.forge.starts.is_empty(), "Origin Kept is held");
        assert!(engine.values.identities.newest(1).is_none());
        let Some(StoreEvent::Kept { owner }) = engine.store.above.pop() else {
            panic!("durable Origin");
        };
        answer(engine, StoreEvent::Kept { owner });
        assert_eq!(engine.forge.starts.get(0).unwrap().1.user, 42);
        assert!(engine.forge.accepted.is_empty(), "Forgejo has no installation set");
        let (generation, value) = engine.values.identities.newest(1).unwrap();
        assert_eq!(generation, 1);
        let value = fixtures::decoded_identity(value);
        assert_eq!(value.login(), if fake { b"temper".as_slice() } else { b"git-user" });
        assert_eq!(
            match value.kind() {
                temper_protocol_worker_wire::v1::Kind::Forgejo(token) => token.token(),
                temper_protocol_worker_wire::v1::Kind::Github(_) => panic!("Forgejo identity"),
            },
            &Secret::new(b"test-git".as_slice().into())
        );
        let mut credentials = 0_u32;
        for event in &engine.events {
            if let root::Event::Credential { number, generation, .. } = event {
                assert_eq!((*number, *generation), (1, 1));
                credentials = credentials.checked_add(1).unwrap();
            }
        }
        assert_eq!(credentials, 1);
        lifecycle::connected(engine, 1);
        let mut connected = false;
        for event in &engine.events {
            if let root::Event::Forge(value) = event
                && let temper_engine_domain_forge::Event::Connection {
                    forge: 1,
                    state: temper_engine_domain_forge::Connection::Connected { id, login },
                } = value.as_ref()
            {
                assert_eq!(*id, 42);
                assert_eq!(login.as_ref(), b"temper");
                connected = true;
            }
        }
        assert!(connected);
        assert!(engine.notes.is_empty(), "no credentials in Notes");
        no_setup_events(engine);
        advance(engine);
        assert!(engine.setup.runs.is_empty(), "Forgejo does not reconcile from Connected");
        // A second start uses exactly the durable store, including its AdminToken and Origin.
        let mut store = core::mem::replace(&mut engine.store, fixtures::records_store());
        for _ in 0..store.above.len() {
            let _terminal = store.above.pop();
        }
        let mut restarted = local_service(local_config(true, fake), store);
        restore_local(&mut restarted);
        let mut face = Face::new();
        local_sent(&mut restarted, &mut face, b"GET /api/v1/user ");
        assert!(contains(&face.wire.received, b"test-admin"));
    }
}

#[test]
fn local_given_uses_the_repository_capacity_no_orgs_and_configured_listener() {
    let mut config = local_config(true, false);
    config.webhooks =
        Some(skein_forge_connection::hooks::Webhooks { listener: skein_io::Addr::Ip(([127, 0, 0, 1], 7101).into()) });
    let (endpoints, _) = crate::forge::prepare(&config, fixtures::forge_runtime()).unwrap();
    assert!(endpoints.forges.get(0).unwrap().access.is_none());
    let (host, _) = settings::fixed(&config, &endpoints).unwrap();
    let entry = host.forges.get(0).unwrap();
    assert_eq!(entry.address, endpoints.forges.get(0).unwrap().address);
    let host::Kind::Forgejo(given) = &entry.kind else {
        panic!("local Given");
    };
    assert_eq!(given.limits.repositories, config.limits.forge.client.repositories);
    assert_eq!(given.limits.repositories, 3);
    assert!(given.orgs.is_empty());
    assert_eq!(given.org_owner.as_deref(), Some(b"owner".as_slice()));
    assert_eq!(given.listener, Some(([127, 0, 0, 1], 7101).into()));
    assert_eq!(prepare(&config, &mut { endpoints }, &fixtures::seeds().crypto, 2, false).unwrap().served.len(), 1);
}
#[test]
fn an_old_form_owned_start_makes_no_reconcile() {
    for token in [false, true] {
        let mut service = local_service(local_config(false, false), local_store(token));
        restore_local(&mut service);
        let engine = service.engine.as_ref().unwrap();
        assert!(engine.setup.lifecycle.local(1).is_none());
        assert!(engine.setup.runs.is_empty());
        assert!(engine.setup.io.is_empty());
        assert!(engine.setup.writes.is_empty(), "no AdminToken Keep");
        assert!(engine.setup.lifecycle.adoptions.is_empty(), "no handover use");
        assert!(engine.notes.is_empty());
        assert!(engine.values.identities.newest(1).is_some(), "retained file identity");
    }
}
fn local_with_other(config: crate::settings::Engine, store: crate::store::Store) -> crate::Service {
    let mut config = config;
    let mut other = fixtures::forges().into_iter().next().unwrap();
    other.number = 2;
    other.kind = crate::settings::Kind::GitHub;
    other.api_token = None;
    other.api = jig_protocol_config::Address::Addr(skein_io::Addr::Ip(([127, 0, 0, 1], 3000).into()));
    config.forges = Box::new([config.forges.into_iter().next().unwrap(), other]);
    let mut runtime = List::with_capacity(2);
    runtime.push(fixtures::forge_runtime().into_boxed().into_iter().next().unwrap()).unwrap();
    let mut other = fixtures::forge_runtime().into_boxed().into_iter().next().unwrap();
    other.number = 2;
    other.address = ([127, 0, 0, 1], 3000).into();
    runtime.push(other).unwrap();
    let mut service = crate::Service::engine(config, &fixtures::seeds(), None, runtime).unwrap();
    service.engine.as_mut().unwrap().store = store;
    service
}
fn other_progresses(service: &mut crate::Service) {
    while service.notes().pop().is_some() {}
    start(service, 77, host::Plan::Manifest { forge: 2, target: Target::Personal });
    let root::SetupEnd::Done { forge: 2, .. } = terminal(service) else {
        panic!("other forge progresses");
    };
    for (_, write) in &service.engine.as_ref().unwrap().setup.writes {
        assert_ne!(write.number, records::number(1, Item::AdminToken));
    }
}
#[test]
fn no_kept_admin_token_names_preparation_and_leaves_the_other_forge_served() {
    let mut service = local_with_other(local_config(true, false), local_store(false));
    restore_local(&mut service);
    let mut prepared = false;
    for note in service.notes().iter() {
        if let crate::Note::ForgeSetupStopped { number: 1, why: crate::SetupStopped::NotPrepared } = note {
            prepared = true;
        }
    }
    assert!(prepared);
    assert!(service.engine.as_ref().unwrap().setup.io.is_empty());
    assert!(service.engine.as_ref().unwrap().forge.starts.is_empty());
    other_progresses(&mut service);
}
#[test]
fn local_stops_missing_busy_and_origin_mismatch_leave_the_other_forge_served() {
    for fault in [0_u8, 1, 2, 3, 4, 5] {
        let mut store = local_store(true);
        if fault == 3 || fault == 4 {
            let origin = records::origin(
                if fault == 3 {
                    skein_forge_connection::settings::Kind::GitHub
                } else {
                    skein_forge_connection::settings::Kind::Forgejo
                },
                &jig_protocol_config::Address::Addr(skein_io::Addr::Ip(([127, 0, 0, 1], 10).into())),
            )
            .encode();
            kept_record(&mut store, 1, Item::Origin, &origin);
        }
        if fault == 5 {
            kept_record(&mut store, 1, Item::AdminToken, b"");
        }
        let mut service = local_with_other(local_config(true, false), store);
        // Hold root restore while setting the host's actual admission conditions.
        for _ in 0_u32..8 {
            crate::forge::records::advance(service.engine.as_mut().unwrap());
        }
        advance(service.engine.as_mut().unwrap());
        if fault == 1 {
            service.engine.as_mut().unwrap().setup.served.remove(&1);
        }
        if fault == 2 {
            service
                .engine
                .as_mut()
                .unwrap()
                .setup
                .runs
                .insert(
                    Token::new(76),
                    Run {
                        first: None,
                        forge: 1,
                        kind: Kind::Adopt,
                        internal: true,
                        previous: temper_engine_domain_forge::Connection::NotConnected,
                    },
                )
                .unwrap();
        }
        restore_local(&mut service);
        if fault == 0 {
            let mut face = Face::new();
            local_sent(&mut service, &mut face, b"GET /api/v1/user ");
            face.wire.write(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n");
            for _ in 0_u32..1024 {
                face.turn(&mut service);
            }
        }
        let expected = match fault {
            0 => Some(crate::SetupStopped::Stopped {
                plan: crate::SetupPlan::Reconcile,
                step: crate::SetupStep { stage: reconcile::Stage::Admin, why: reconcile::Why::Refused(403) },
            }),
            1 => Some(crate::SetupStopped::Missing),
            2 => Some(crate::SetupStopped::Busy),
            _ => None,
        };
        let engine = service.engine.as_ref().unwrap();
        let mut named = false;
        for note in &engine.notes {
            named |= match (note, expected.as_ref()) {
                (crate::Note::ForgeSetupStopped { number: 1, why }, Some(expected)) => why == expected,
                (crate::Note::ForgeNotConnected { number: 1, why: records::NotConnected::OtherForge { .. } }, None) => {
                    fault != 5
                }
                (crate::Note::ForgeNotConnected { number: 1, why: records::NotConnected::Unreadable }, None) => {
                    fault == 5
                }
                _ => false,
            };
        }
        assert!(named);
        if fault != 0 {
            assert!(engine.setup.io.is_empty(), "refused before any request");
        }
        assert!(engine.forge.starts.is_empty());
        assert!(engine.setup.writes.is_empty());
        no_setup_events(engine);
        other_progresses(&mut service);
    }
}

fn no_setup_events(engine: &crate::engine::Engine) {
    for event in &engine.events {
        if let root::Event::Setup { .. } = event {
            panic!("admin and setup stop text never enter root events");
        }
    }
}

fn told_local(engine: &crate::engine::Engine, expected: &domain::Connection) -> bool {
    for event in &engine.events {
        if let root::Event::Forge(event) = event
            && let domain::Event::Connection { forge: 1, state } = event.as_ref()
            && state == expected
        {
            return true;
        }
    }
    false
}
fn with_handover(token: &[u8]) -> crate::settings::Engine {
    let mut config = local_config(true, false);
    config.setup[0].handover = Some(Secret::new(token.into()));
    config
}
fn takeover_to_kept(service: &mut crate::Service, face: &mut Face) {
    local_sent(service, face, b"GET /api/v1/user ");
    face.reply(service, br#"{"id":1,"login":"temper-admin","is_admin":true}"#);
    let engine = service.engine.as_mut().unwrap();
    assert_eq!(engine.setup.writes.len(), 1);
    assert_eq!(engine.setup.writes.first().unwrap().1.number, records::number(1, Item::AdminToken));
    assert!(service.take_handover_removal().is_none(), "no removal before Kept");
    let engine = service.engine.as_mut().unwrap();
    assert!(engine.setup.answers.is_empty(), "Written waits for Kept");
    let Some(StoreEvent::Kept { owner }) = engine.store.above.pop() else { panic!("durable AdminToken") };
    answer(engine, StoreEvent::Kept { owner });
    let Some((_, host::Answer::Written)) = engine.setup.answers.pop() else { panic!("Kept answers Written") };
    assert!(service.take_handover_removal().is_none(), "Kept alone does not remove");
    local_sent(service, face, b"GET /api/v1/settings/api ");
    face.reply(service, br#"{"max_response_items":50,"default_paging_num":10}"#);
    local_sent(service, face, b"GET /api/v1/admin/users/temper-admin/tokens");
}
fn takeover_done(service: &mut crate::Service, face: &mut Face) -> crate::HandoverRemoval {
    takeover_to_kept(service, face);
    face.hold_done = true;
    face.reply(service, b"[]");
    assert!(face.done.is_some());
    assert!(service.take_handover_removal().is_none(), "held Done blocks removal");
    service.engine.as_mut().unwrap().setup.above.push(face.done.take().unwrap());
    advance(service.engine.as_mut().unwrap());
    let remove = service.take_handover_removal().expect("Done requests removal");
    assert!(service.take_handover_removal().is_none(), "one correlated removal");
    assert!(service.engine.as_ref().unwrap().forge.starts.is_empty());
    remove
}
fn box_started(service: &mut crate::Service) -> crate::HandoverRead {
    while service.notes().pop().is_some() {}
    let engine = service.engine.as_mut().unwrap();
    let mut out = Queue::with_capacity(3);
    engine.box_link = crate::route::box_link::Link::new(128, true);
    engine.box_link.ask(1, Token::new(456), &mut out);
    crate::route::box_link::drain(engine, &mut out);
    engine.box_link.write = None;
    engine.box_link.read(b"started 1\n", &mut out);
    crate::route::box_link::drain(engine, &mut out);
    assert!(told_local(engine, &domain::Connection::Connecting));
    service.take_handover_read().expect("box preparation requests the handover")
}
fn finish_local(service: &mut crate::Service, face: &mut Face) {
    face.hold_done = false;
    reconcile_to_repository(service, face);
    face.reply(service, br#"{"permission":"write","user":{"id":42,"login":"temper"}}"#);
    let engine = service.engine.as_mut().unwrap();
    assert!(engine.forge.starts.is_empty());
    let Some(StoreEvent::Kept { owner }) = engine.store.above.pop() else { panic!("durable Origin") };
    answer(engine, StoreEvent::Kept { owner });
    assert_eq!(engine.forge.starts.get(0).unwrap().1.user, 42);
    lifecycle::connected(engine, 1);
}
#[test]
fn takeover_waits_for_kept_done_and_correlated_removal_before_restored_reconcile() {
    let mut service = local_with_other(with_handover(b"test-admin"), local_store(false));
    restore_local(&mut service);
    let mut face = Face::new();
    let remove = takeover_done(&mut service, &mut face);
    other_progresses(&mut service);
    service.handover_removed(2, remove.run, Ok(()));
    advance(service.engine.as_mut().unwrap());
    service.handover_removed(1, Token::new(0), Ok(()));
    advance(service.engine.as_mut().unwrap());
    let lifecycle::FileStep::Removing { .. } = service.engine.as_ref().unwrap().setup.lifecycle.local(1).unwrap().file
    else {
        panic!("unrelated removal answers change nothing")
    };
    service.handover_removed(1, remove.run, Ok(()));
    advance(service.engine.as_mut().unwrap());
    finish_local(&mut service, &mut face);
    let engine = service.engine.as_mut().unwrap();
    assert_eq!(engine.values.identities.newest(1).unwrap().0, 1);
    engine.forge.starts.clear();
    engine.forge_records.rows.push(
        records::number(1, Item::GitToken),
        Secret::new(records::encode(b"replacement-git").into_boxed_slice()),
        engine.setup.value_bytes,
    );
    lifecycle::connect_local(engine, 1);
    assert_eq!(engine.values.identities.newest(1).unwrap().0, 2, "every local connect puts the current git token");
    let identity = fixtures::decoded_identity(engine.values.identities.newest(1).unwrap().1);
    let temper_protocol_worker_wire::v1::Kind::Forgejo(token) = identity.kind() else { panic!("local git identity") };
    assert!(token.token().expose() == b"replacement-git", "the next generation serves the replacement");
    for generation in [1, 2] {
        let mut count = 0_u32;
        for event in &engine.events {
            if let root::Event::Credential { number: 1, generation: found, .. } = event
                && *found == generation
            {
                count = count.checked_add(1).unwrap();
            }
        }
        assert_eq!(count, 1);
    }
}
#[test]
fn restart_always_takes_a_present_handover_even_when_the_record_differs() {
    for next in [Some(b"test-admin".as_slice()), Some(b"different-admin".as_slice()), None] {
        let mut service = local_service(with_handover(b"test-admin"), local_store(false));
        restore_local(&mut service);
        let mut face = Face::new();
        let remove = takeover_done(&mut service, &mut face);
        service.handover_removed(1, remove.run, Err(21_i32));
        advance(service.engine.as_mut().unwrap());
        let engine = service.engine.as_mut().unwrap();
        assert!(engine.forge.starts.is_empty());
        assert!(engine.setup.runs.is_empty(), "failed removal blocks reconcile");
        let store = core::mem::replace(&mut engine.store, fixtures::records_store());
        let config = match next {
            Some(value) => with_handover(value),
            None => local_config(true, false),
        };
        let mut restarted = local_service(config, store);
        restore_local(&mut restarted);
        let engine = restarted.engine.as_ref().unwrap();
        let kind = engine.setup.runs.first().unwrap().1.kind;
        assert_eq!(kind, if next.is_some() { Kind::LocalTakeOver } else { Kind::LocalReconcile });
        if let Some(value) = next {
            let mut face = Face::new();
            let remove = takeover_done(&mut restarted, &mut face);
            let engine = restarted.engine.as_ref().unwrap();
            assert!(
                engine.forge_records.rows.forge(1).unwrap().get(Item::AdminToken).unwrap() == value,
                "the present handover replaces the record"
            );
            restarted.handover_removed(1, remove.run, Ok(()));
            advance(restarted.engine.as_mut().unwrap());
            assert_eq!(restarted.engine.as_ref().unwrap().setup.runs.first().unwrap().1.kind, Kind::LocalReconcile);
        }
    }
}
#[test]
fn an_asked_forge_takes_its_handover_at_started_and_connects() {
    let mut config = local_config(true, false);
    config.setup[0].entry.asked = true;
    let mut service = local_service(config, local_store(false));
    restore_local(&mut service);
    assert!(!service.engine.as_ref().unwrap().setup.lifecycle.local(1).unwrap().pending);
    let engine = service.engine.as_ref().unwrap();
    assert!(
        told_local(engine, &domain::Connection::NotSetUp)
            || engine.domain.forge_connection(1) == domain::Connection::NotSetUp,
        "the stopped local start preserves the client's setup offer"
    );
    let read = box_started(&mut service);
    assert!(service.take_handover_read().is_none());
    service.handover_read(1, read.run, Ok(Some(Secret::new(b"test-admin".as_slice().into()))));
    advance(service.engine.as_mut().unwrap());
    let mut face = Face::new();
    let remove = takeover_done(&mut service, &mut face);
    service.handover_removed(1, remove.run, Ok(()));
    advance(service.engine.as_mut().unwrap());
    finish_local(&mut service, &mut face);
    let engine = service.engine.as_ref().unwrap();
    assert_eq!(engine.values.identities.newest(1).unwrap().0, 1);
    assert!(told_local(engine, &domain::Connection::Connected { id: 42, login: b"temper".as_slice().into() }));
}
#[test]
fn a_takeover_that_stops_at_started_fails_naming_its_step_and_keeps_the_file() {
    let mut service = local_with_other(local_config(true, false), local_store(false));
    restore_local(&mut service);
    let read = box_started(&mut service);
    service.handover_read(1, read.run, Ok(Some(Secret::new(Box::new([])))));
    advance(service.engine.as_mut().unwrap());
    advance(service.engine.as_mut().unwrap());
    assert!(service.take_handover_removal().is_none());
    let engine = service.engine.as_ref().unwrap();
    assert!(told_local(
        engine,
        &domain::Connection::SetUpFailed { why: b"taking the handover: Admin: Missing".as_slice().into() }
    ));
    assert!(engine.forge.starts.is_empty());
    other_progresses(&mut service);
}
#[test]
fn a_failed_removal_at_started_blocks_reconcile_and_the_next_ask_takes_it_over_again() {
    let mut service = local_service(local_config(true, false), local_store(false));
    restore_local(&mut service);
    for attempt in 0_u32..2 {
        let read = box_started(&mut service);
        service.handover_read(1, read.run, Ok(Some(Secret::new(b"test-admin".as_slice().into()))));
        advance(service.engine.as_mut().unwrap());
        let mut face = Face::new();
        let remove = takeover_done(&mut service, &mut face);
        service.handover_removed(1, remove.run, if attempt == 1 { Ok(()) } else { Err(21_i32) });
        advance(service.engine.as_mut().unwrap());
        let engine = service.engine.as_ref().unwrap();
        assert!(engine.forge.starts.is_empty());
        if attempt == 0 {
            assert!(engine.setup.runs.is_empty());
            assert!(told_local(
                engine,
                &domain::Connection::SetUpFailed {
                    why: b"removing the handover: forge 1's admin_token: errno 21".as_slice().into()
                }
            ));
        } else {
            assert_eq!(engine.setup.runs.first().unwrap().1.kind, Kind::LocalReconcile);
        }
    }
}
#[test]
fn takeover_stops_missing_and_busy_keep_the_handover_and_isolate_the_forge() {
    for fault in 0_u8..4 {
        let config = if fault == 1 { with_handover(b"") } else { with_handover(b"test-admin") };
        let mut service = local_with_other(config, local_store(false));
        for _ in 0_u32..8 {
            crate::forge::records::advance(service.engine.as_mut().unwrap());
        }
        advance(service.engine.as_mut().unwrap());
        if fault == 2 {
            service.engine.as_mut().unwrap().setup.served.remove(&1);
        }
        if fault == 3 {
            service
                .engine
                .as_mut()
                .unwrap()
                .setup
                .runs
                .insert(
                    Token::new(76),
                    Run {
                        first: None,
                        forge: 1,
                        kind: Kind::Adopt,
                        internal: true,
                        previous: temper_engine_domain_forge::Connection::NotConnected,
                    },
                )
                .unwrap();
        }
        restore_local(&mut service);
        if fault == 0 {
            let mut face = Face::new();
            local_sent(&mut service, &mut face, b"GET /api/v1/user ");
            face.wire.write(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n");
            for _ in 0_u32..1024 {
                face.turn(&mut service);
            }
        }
        let expected = match fault {
            0 | 1 => crate::SetupStopped::Stopped {
                plan: crate::SetupPlan::TakeOver,
                step: crate::SetupStep {
                    stage: reconcile::Stage::Admin,
                    why: if fault == 0 { reconcile::Why::Refused(403) } else { reconcile::Why::Missing },
                },
            },
            2 => crate::SetupStopped::Missing,
            3 => crate::SetupStopped::Busy,
            _ => unreachable!(),
        };
        let engine = service.engine.as_ref().unwrap();
        let mut named = false;
        for note in &engine.notes {
            if let crate::Note::ForgeSetupStopped { number: 1, why } = note {
                named |= *why == expected;
            }
        }
        assert!(named, "fault {fault} has its typed failure");
        assert!(engine.forge.starts.is_empty());
        assert!(engine.setup.lifecycle.local(1).unwrap().handover.is_none(), "stops discard the staged secret");
        assert!(service.take_handover_removal().is_none(), "stops retain the file");
        other_progresses(&mut service);
    }
}
#[test]
fn unreadable_or_missing_handover_at_started_is_a_named_retryable_failure() {
    for answer in [Ok(None), Err(crate::HandoverUnread::Unsafe(skein_io::file::Unsafe::Link))] {
        let mut service = local_service(local_config(true, false), local_store(false));
        restore_local(&mut service);
        let read = box_started(&mut service);
        service.handover_read(1, read.run, answer);
        advance(service.engine.as_mut().unwrap());
        let engine = service.engine.as_ref().unwrap();
        let mut failed = false;
        for event in &engine.events {
            if let root::Event::Forge(event) = event
                && let domain::Event::Connection { forge: 1, state: domain::Connection::SetUpFailed { why } } =
                    event.as_ref()
            {
                failed |= why.as_ref() == b"the box's preparation left no handover"
                    || why.starts_with(b"the handover is unreadable: forge 1's admin_token: ");
            }
        }
        assert!(failed);
        assert!(engine.setup.runs.is_empty());
        assert!(service.take_handover_removal().is_none());
        let read = box_started(&mut service);
        service.handover_read(1, read.run, Ok(Some(Secret::new(b"test-admin".as_slice().into()))));
        advance(service.engine.as_mut().unwrap());
        assert_eq!(service.engine.as_ref().unwrap().setup.runs.first().unwrap().1.kind, Kind::LocalTakeOver);
    }
}
#[test]
fn closing_discards_pending_file_correlations_and_late_answers() {
    for removal in [false, true] {
        let mut service = local_service(local_config(true, false), local_store(false));
        restore_local(&mut service);
        let read = box_started(&mut service);
        let run = if removal {
            service.handover_read(1, read.run, Ok(Some(Secret::new(b"test-admin".as_slice().into()))));
            advance(service.engine.as_mut().unwrap());
            takeover_done(&mut service, &mut Face::new()).run
        } else {
            read.run
        };
        service.engine.as_mut().unwrap().setup.close();
        service.handover_read(1, run, Ok(Some(Secret::new(b"test-admin".as_slice().into()))));
        advance(service.engine.as_mut().unwrap());
        service.handover_removed(1, run, Ok(()));
        advance(service.engine.as_mut().unwrap());
        assert!(service.take_handover_read().is_none());
        assert!(service.take_handover_removal().is_none());
        assert!(service.engine.as_ref().unwrap().forge.starts.is_empty());
    }
}

#[test]
#[expect(clippy::disallowed_methods, reason = "the diagnostic protocol test checks UTF-8 after truncation")]
fn setup_reasons_fit_the_wire_at_a_character_boundary() {
    let mut text = skein_lib::Writer::new(257);
    text.put(&[b'x'; 255]).unwrap();
    text.put("é".as_bytes()).unwrap();
    for (text, length) in [
        (Box::from([b'x'; 255].as_slice()), 255),
        (Box::from([b'x'; 256].as_slice()), 256),
        (Box::from([b'x'; 257].as_slice()), 256),
        (text.finish(), 255),
    ] {
        let reason = lifecycle::bounded_reason(text);
        assert_eq!(reason.len(), length);
        core::str::from_utf8(&reason).unwrap();
    }
}

#[test]
fn file_answers_wait_for_output_room_and_ignore_other_correlations() {
    let mut service = local_service(local_config(true, false), local_store(false));
    restore_local(&mut service);
    let read = box_started(&mut service);
    service.handover_read(2, read.run, Ok(None));
    service.handover_read(1, Token::new(0), Ok(None));
    let lifecycle::FileStep::Reading { sent: true, .. } =
        service.engine.as_ref().unwrap().setup.lifecycle.local(1).unwrap().file
    else {
        panic!("unrelated read answers change nothing")
    };
    let engine = service.engine.as_mut().unwrap();
    while engine.notes.room() > 0 {
        engine.notes.push(crate::Note::ForgeSetupStopped { number: 2, why: crate::SetupStopped::Busy });
    }
    service.handover_read(1, read.run, Ok(Some(Secret::new(b"test-admin".as_slice().into()))));
    advance(service.engine.as_mut().unwrap());
    assert!(service.engine.as_ref().unwrap().setup.runs.is_empty(), "read waits for output room");
    while service.notes().pop().is_some() {}
    advance(service.engine.as_mut().unwrap());
    let remove = takeover_done(&mut service, &mut Face::new());
    let engine = service.engine.as_mut().unwrap();
    while engine.notes.room() > 0 {
        engine.notes.push(crate::Note::ForgeSetupStopped { number: 2, why: crate::SetupStopped::Busy });
    }
    service.handover_removed(1, remove.run, Ok(()));
    advance(service.engine.as_mut().unwrap());
    assert_eq!(service.engine.as_ref().unwrap().setup.runs.first().unwrap().1.kind, Kind::LocalRemoval);
    while service.notes().pop().is_some() {}
    advance(service.engine.as_mut().unwrap());
    assert_eq!(service.engine.as_ref().unwrap().setup.runs.first().unwrap().1.kind, Kind::LocalReconcile);
}

#[test]
fn a_missing_file_at_started_reconciles_the_kept_token_and_names_a_stop() {
    let mut service = local_with_other(local_config(true, false), local_store(true));
    restore_local(&mut service);
    let mut face = Face::new();
    for asked in [false, true] {
        if asked {
            let read = box_started(&mut service);
            service.handover_read(1, read.run, Ok(None));
            advance(service.engine.as_mut().unwrap());
        }
        assert_eq!(service.engine.as_ref().unwrap().setup.runs.first().unwrap().1.kind, Kind::LocalReconcile);
        assert!(service.take_handover_removal().is_none());
        local_sent(&mut service, &mut face, b"GET /api/v1/user ");
        face.wire.write(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n");
        for _ in 0_u32..1024 {
            face.turn(&mut service);
        }
    }
    assert!(told_local(
        service.engine.as_ref().unwrap(),
        &domain::Connection::SetUpFailed { why: b"reconciling: Admin: Refused(403)".as_slice().into() }
    ));
    other_progresses(&mut service);
}

#[path = "setup_bound_webhook.rs"]
mod bound_webhook;

#[path = "setup_admin.rs"]
mod admin_tests;

#[path = "setup_create.rs"]
mod create_tests;

#[test]
fn the_demo_waits_for_started_only_under_the_box_even_with_retained_access() {
    for (under_box, retained) in [(false, false), (false, true), (true, false), (true, true)] {
        let mut config = local_config(true, true);
        config.demo = Some(crate::settings::Demo {
            project: 1,
            forge: 1,
            provider: 3,
            owner: b"demo".as_slice().into(),
            name: b"widget".as_slice().into(),
            cues: Box::new([]),
            landing: b"main".as_slice().into(),
            key_file: b"demo/llm.key".as_slice().into(),
            state: None,
        });
        config.setup[0].handover = Some(Secret::new(b"stale-handover".as_slice().into()));
        let configured = crate::startup::application::configured(&config);
        assert!(configured[0].asked);
        assert_eq!(configured[0].mode, domain::Mode::Fake);
        let (endpoints, roots) = crate::forge::prepare(&config, fixtures::forge_runtime()).unwrap();
        let owned = skein_forge_connection::settings::owned_bytes(&endpoints).unwrap();
        let io = config.jig.limits.io;
        let mut service = crate::Service::prepared(
            config,
            &fixtures::seeds(),
            None,
            (endpoints, roots, owned),
            None,
            0,
            io,
            under_box,
        )
        .unwrap();
        service.engine.as_mut().unwrap().store = local_store(retained);
        restore_local(&mut service);
        let engine = service.engine.as_ref().unwrap();
        if !under_box {
            assert_eq!(
                engine.setup.runs.first().unwrap().1.kind,
                Kind::LocalTakeOver,
                "without the box, the runner has already prepared the fake"
            );
            continue;
        }
        assert!(
            told_local(engine, &domain::Connection::NotSetUp)
                || engine.domain.forge_connection(1) == domain::Connection::NotSetUp
        );
        assert_eq!(engine.domain.forges().rows[0].offer, domain::Offer::NotOffered);
        assert!(engine.setup.runs.is_empty());
        assert!(engine.forge.starts.is_empty());
        assert!(engine.notes.is_empty());
        let read = box_started(&mut service);
        service.handover_read(1, read.run, Ok(Some(Secret::new(b"test-admin".as_slice().into()))));
        advance(service.engine.as_mut().unwrap());
        let mut face = Face::new();
        let remove = takeover_done(&mut service, &mut face);
        service.handover_removed(1, remove.run, Ok(()));
        advance(service.engine.as_mut().unwrap());
        finish_local(&mut service, &mut face);
        let engine = service.engine.as_ref().unwrap();
        assert!(told_local(engine, &domain::Connection::Connected { id: 42, login: b"temper".as_slice().into() }));
    }
}

fn demo_config() -> crate::settings::Engine {
    let mut config = local_config(true, true);
    config.jig.limits.core.people.act_grants = 1;
    config.jig.acts = Box::new([jig_domain_people::ActGrant {
        connector: 1,
        may: Box::new([
            jig_domain_people::ActKind::Close,
            jig_domain_people::ActKind::AddKey,
            jig_domain_people::ActKind::RemoveAccount,
            jig_domain_people::ActKind::Adopt,
        ]),
    }]);
    config.demo = Some(crate::settings::Demo {
        project: 1,
        forge: 1,
        provider: 3,
        owner: b"demo".as_slice().into(),
        name: b"widget".as_slice().into(),
        cues: Box::new([]),
        landing: b"main".as_slice().into(),
        key_file: b"demo/llm.key".as_slice().into(),
        state: None,
    });
    config
}
fn ended_demo_store() -> crate::store::Store {
    let mut store = local_store(true);
    fixtures::commit(
        &mut store,
        2,
        Box::new([root::Write::Save(root::Record::Forge {
            row: Box::new(domain::Stored::Demo(domain::Demo { forge: 1, state: domain::DemoState::Ended })),
        })]),
    );
    assert!(store.above.pop().is_some());
    store
}
#[test]
fn restored_ended_demo_without_box_skips_setup_adoption_connect_and_handover() {
    let mut config = demo_config();
    config.setup[0].handover = Some(Secret::new(b"test-admin".as_slice().into()));
    let mut service = local_service(config, ended_demo_store());
    restore_local(&mut service);
    let engine = service.engine.as_mut().unwrap();
    assert_eq!(engine.demo.as_ref().unwrap().state, Some(domain::DemoState::Ended));
    assert!(engine.domain.forge_ended(1));
    // A stale file or a pending start-time adoption cannot make the scan ready.
    engine
        .setup
        .lifecycle
        .adoptions
        .insert(1, lifecycle::Adoption { next: core::num::NonZeroU64::new(1).unwrap(), repositories: 2, files: None })
        .unwrap();
    assert!(!engine.setup.lifecycle.ready(&engine.setup.runs, engine.setup.env.limits.runs, &engine.domain));
    for _ in 0_u8..4 {
        advance(engine);
    }
    assert!(engine.setup.runs.is_empty());
    assert!(engine.setup.io.is_empty());
    assert!(engine.forge.starts.is_empty());
    lifecycle::started(engine, 1);
    lifecycle::connect_local(engine, 1);
    assert!(service.take_handover_read().is_none());
    assert!(service.take_handover_removal().is_none());
}
#[test]
fn an_end_during_handover_or_reconcile_cannot_continue_setup() {
    for during_reconcile in [false, true] {
        let mut config = demo_config();
        config.setup[0].handover = Some(Secret::new(b"test-admin".as_slice().into()));
        let mut service = local_service(config, local_store(true));
        restore_local(&mut service);
        let engine = service.engine.as_mut().unwrap();
        let run = *engine.setup.runs.first().unwrap().0;
        if during_reconcile {
            engine.setup.runs.get_mut(&run).unwrap().kind = Kind::LocalReconcile;
        }
        engine.demo.as_mut().unwrap().state = Some(domain::DemoState::Ended);
        lifecycle::done(engine, run, reconcile::Outcome::Nothing);
        assert!(engine.setup.runs.is_empty());
        assert!(engine.setup.writes.is_empty());
        assert!(engine.forge.starts.is_empty());
        assert!(service.take_handover_removal().is_none());
    }
}
#[test]
fn ended_forge_calls_are_answered_locally_at_zero_cost_and_hints_are_dropped() {
    let mut service = local_service(demo_config(), ended_demo_store());
    restore_local(&mut service);
    let engine = service.engine.as_mut().unwrap();
    while engine.events.pop().is_some() {}
    let call = Token::new(9001);
    let repository = skein_forge::api::Repository { forge: 1, repository: 41 };
    crate::route::request(
        engine,
        root::Request::Forge {
            call,
            repository,
            op: skein_forge::api::Op::Read(skein_forge::api::Read::Permission { user: 42 }),
        },
    );
    let Some(root::Event::Forge(event)) = engine.events.pop() else { panic!("immediate local answer") };
    assert_eq!(
        *event,
        domain::Event::Client(skein_forge_client::Event::Answered {
            call,
            cost: 0,
            free: 0,
            rate: None,
            result: Err(skein_forge::api::Error::NotConnected),
        })
    );
    assert!(engine.forge.above.is_empty());
    engine.forge.above.push(skein_forge_connection::Event::Hint(skein_forge_connection::hooks::Hinted {
        connection: Token::new(9002),
        hint: skein_forge::api::Hint { repository, change: skein_forge::api::Change::Item(7), key: None },
    }));
    crate::engine::forge_pass(engine);
    assert!(engine.events.is_empty());
    assert!(engine.forge.above.is_empty());
    assert!(engine.forge.starts.is_empty());
    assert!(engine.forge.io.is_empty());
}
#[test]
fn github_app_urls_use_the_clients_public_origin() {
    let public = crate::startup::application::Origin {
        scheme: crate::startup::application::Scheme::Https,
        host: Box::from(b"temper.example.com".as_slice()),
        port: Some(8443),
        name: Box::from(b"temper.example.com:8443".as_slice()),
    };
    let mut service = routing_service_at(8, Duration::from_secs(3600), Some(public));
    let state = manifest(&mut service, 1);
    let page = service.setup_page(1, &state, Time::ZERO).unwrap();
    assert!(contains(page, b"https://temper.example.com:8443/forges/1/github/redirect"));
    assert!(contains(page, b"https://temper.example.com:8443/forges/1/github/setup"));
}
