//! Loopback HTTP socket ownership. TLS servers stay with an external owner;
//! this acceptor never treats off-host encrypted bytes as plaintext HTTP.
use crate::{
    config::Config,
    connection::{self, Connection},
    service::Service,
};
use skein_io::{self as io, kernel::Addr};
use skein_lib::{Env, Id, Queue, Set, Slab, Token, stream};
use temper_fake_forge_domain as domain;
pub const OWNER: Token = Token::new(u64::MAX);
pub const MAX_UP: u32 = 1;
pub const MAX_DOWN: u32 = connection::MAX_DOWN + 1;
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub connections: u32,
    pub connection: connection::Limits,
}
#[derive(Debug)]
pub enum Notice {
    Call(domain::Event),
    Hook(domain::Request),
    Listening { addr: Addr },
    Failed { error: io::Error },
    Closed,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Idle,
    Starting,
    Listening,
    Closing,
    Closed,
}
#[expect(missing_debug_implementations, reason = "connection scratch may briefly retain authentication headers")]
struct Entry {
    socket: Token,
    connection: Option<Connection>,
    requested: bool,
    closing: bool,
}
#[expect(missing_debug_implementations, reason = "service configuration contains API tokens")]
pub struct Acceptor {
    service: Service,
    entries: Slab<Entry>,
    bound: Set<Id<Entry>>,
    ready: Set<Id<Entry>>,
    state: State,
    listener: Option<Token>,
    listener_closed: bool,
    held: u32,
    upper: Queue<connection::Event>,
    lower: Queue<stream::Down>,
}
impl Acceptor {
    #[must_use]
    pub fn new(config: Config, limits: &Limits, io_limits: &io::Limits) -> Option<Acceptor> {
        if limits.connections == 0 || !fits_io(limits, io_limits) {
            return None;
        }
        Some(Acceptor {
            service: Service::new(config, &limits.connection)?,
            entries: Slab::with_capacity(limits.connections),
            bound: Set::with_capacity(limits.connections),
            ready: Set::with_capacity(limits.connections),
            state: State::Idle,
            listener: None,
            listener_closed: false,
            held: 0,
            upper: Queue::with_capacity(connection::MAX_UP),
            lower: Queue::with_capacity(connection::MAX_DOWN),
        })
    }
    #[must_use]
    pub const fn connections(&self) -> u32 {
        self.held
    }
    #[must_use]
    pub fn is_ready(&self) -> bool {
        if !self.ready.is_empty() {
            return true;
        }
        if self.state == State::Closing {
            if self.listener_closed && self.held == 0 {
                return true;
            }
            for id in &self.bound {
                if !self.entries.get(*id).expect("bound entry").requested {
                    return true;
                }
            }
        }
        false
    }
    pub fn reclaim(&mut self) {
        self.entries.reclaim();
        self.service.reclaim();
    }
    #[must_use]
    pub fn start(&mut self, addr: Addr, below: &mut Queue<io::Request>) -> bool {
        if self.state != State::Idle || !addr.ip().is_loopback() {
            return false;
        }
        self.state = State::Starting;
        below.push(io::Request::Listen { owner: OWNER, addr });
        true
    }
    pub fn close(&mut self, below: &mut Queue<io::Request>) {
        match self.state {
            State::Idle => {
                self.listener_closed = true;
                self.state = State::Closing;
            }
            State::Starting => self.state = State::Closing,
            State::Listening => {
                self.state = State::Closing;
                below.push(io::Request::Close { entity: self.listener.expect("listening binding") });
            }
            State::Closing | State::Closed => {}
        }
    }
    pub fn up(
        &mut self,
        env: &Env<Limits>,
        event: io::Event,
        above: &mut Queue<Notice>,
        below: &mut Queue<io::Request>,
    ) {
        match event {
            io::Event::Listening { owner, listener, addr } => {
                assert!(owner == OWNER, "listener binding");
                self.listener = Some(listener);
                if self.state == State::Starting {
                    self.state = State::Listening;
                    above.push(Notice::Listening { addr });
                } else {
                    below.push(io::Request::Close { entity: listener });
                }
            }
            io::Event::Accepted { owner, socket, peer: _ } => {
                assert!(owner == OWNER, "listener accepts");
                if self.state != State::Listening || self.entries.is_full() {
                    below.push(io::Request::Reject { socket });
                    return;
                }
                let id = self
                    .entries
                    .insert(Entry { socket, connection: None, requested: false, closing: false })
                    .ok()
                    .expect("checked room");
                self.entries.get_mut(id).expect("inserted").connection =
                    Some(Connection::new(id.token(), &env.limits.connection).expect("startup validation"));
                self.bound.insert(id).expect("connections cap");
                self.ready.insert(id).expect("connections cap");
                self.held = self.held.checked_add(1).expect("cap");
                below.push(io::Request::Bind { socket, owner: id.token() });
            }
            io::Event::Stream { owner, up } => {
                let id = Id::<Entry>::from_token(owner);
                let entry = self.entries.get_mut(id).expect("io binding remains through Closed");
                if !entry.closing {
                    entry.connection.as_mut().expect("bound connection").up(&connection_env(env), up, &mut self.lower);
                    self.drain(id, above, below);
                    self.follow(id);
                }
            }
            io::Event::Closed { owner } => {
                if owner == OWNER {
                    self.listener_closed = true;
                    self.listener = None;
                    if self.state != State::Closed {
                        self.state = State::Closing;
                    }
                } else {
                    let id = Id::<Entry>::from_token(owner);
                    let entry = self.entries.get_mut(id).expect("actual Closed bound entry");
                    entry.connection.as_mut().expect("bound").closed(&mut self.service, &mut self.upper);
                    self.drain(id, above, below);
                    assert!(self.bound.remove(&id), "actual Closed releases a binding");
                    self.ready.remove(&id);
                    self.entries.retire(id);
                    self.held = self.held.checked_sub(1).expect("one binding");
                }
            }
            io::Event::Failed { owner, error } => {
                if owner == OWNER {
                    above.push(Notice::Failed { error });
                    if self.listener.is_some() {
                        self.close(below);
                    } else {
                        self.state = State::Closing;
                    }
                } else {
                    let id = Id::<Entry>::from_token(owner);
                    let entry = self.entries.get_mut(id).expect("bound stream");
                    entry.connection.as_mut().expect("bound").up(
                        &connection_env(env),
                        stream::Up::Failed(stream::Fault::Other),
                        &mut self.lower,
                    );
                    self.drain(id, above, below);
                    self.follow(id);
                }
            }
            io::Event::Connecting { .. } | io::Event::Connected { .. } => unreachable!("acceptor never connects"),
        }
    }
    pub fn resume(
        &mut self,
        env: &Env<Limits>,
        forge: &domain::Domain,
        settings: &domain::Config,
        above: &mut Queue<Notice>,
        below: &mut Queue<io::Request>,
    ) {
        if let Some(id) = self.ready.first().copied() {
            assert!(self.ready.remove(&id), "ready entry taken");
            let entry = self.entries.get_mut(id).expect("ready bound");
            entry.connection.as_mut().expect("bound").resume(
                &connection_env(env),
                &mut self.service,
                forge,
                settings,
                &mut self.upper,
                &mut self.lower,
            );
            self.drain(id, above, below);
            self.follow(id);
            return;
        }
        if self.state == State::Closing {
            if self.listener_closed && self.held == 0 {
                self.state = State::Closed;
                above.push(Notice::Closed);
                return;
            }
            let mut selected = None;
            for id in &self.bound {
                if !self.entries.get(*id).expect("bound").requested {
                    selected = Some(*id);
                    break;
                }
            }
            if let Some(id) = selected {
                let entry = self.entries.get_mut(id).expect("bound");
                entry.requested = true;
                entry.connection.as_mut().expect("bound").close(&mut self.service);
                self.follow(id);
            }
        }
    }
    pub fn down(
        &mut self,
        env: &Env<Limits>,
        request: domain::Request,
        forge: &domain::Domain,
        settings: &domain::Config,
        above: &mut Queue<Notice>,
    ) {
        match request {
            domain::Request::Reply { to, result } => {
                let Some(route) = self.service.route(to) else {
                    return;
                };
                let id = Id::<Entry>::from_token(route.owner);
                let Some(entry) = self.entries.get_mut(id) else {
                    return;
                };
                if !entry.closing {
                    entry.connection.as_mut().expect("bound").reply(
                        &connection_env(env),
                        &mut self.service,
                        forge,
                        settings,
                        route.call,
                        result,
                    );
                    self.follow(id);
                }
            }
            domain::Request::Hook { .. } => above.push(Notice::Hook(request)),
        }
    }
    fn follow(&mut self, id: Id<Entry>) {
        let entry = self.entries.get(id).expect("bound");
        if !entry.closing && entry.connection.as_ref().expect("bound").is_ready() {
            self.ready.insert(id).expect("cap");
        }
    }
    fn drain(&mut self, id: Id<Entry>, above: &mut Queue<Notice>, below: &mut Queue<io::Request>) {
        let socket = self.entries.get(id).expect("bound").socket;
        for _ in 0..connection::MAX_DOWN {
            if let Some(down) = self.lower.pop() {
                below.push(io::Request::Stream { stream: socket, down });
            } else {
                break;
            }
        }
        if let Some(event) = self.upper.pop() {
            match event {
                connection::Event::Call(call) => above.push(Notice::Call(call)),
                connection::Event::Close => {
                    let entry = self.entries.get_mut(id).expect("bound");
                    if !entry.closing {
                        entry.closing = true;
                        self.service.lost(id.token());
                        below.push(io::Request::Close { entity: socket });
                    }
                }
                connection::Event::Closed => {}
            }
        }
    }
}
fn connection_env(env: &Env<Limits>) -> Env<connection::Limits> {
    Env { now: env.now, wall: env.wall, limits: env.limits.connection }
}
#[must_use]
pub fn fits_io(limits: &Limits, io_limits: &io::Limits) -> bool {
    let http = &limits.connection.http;
    limits.connection.valid()
        && io_limits.is_usable()
        && skein_http::server::largest_read(http) <= io_limits.largest_read()
        && skein_http::server::largest_room(http) <= io_limits.largest_room()
        && io_limits.sends.saturating_add(1) >= io_limits.output
}
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    let each = connection::worst_case(&limits.connection)?;
    Slab::<Entry>::worst_case(limits.connections)?
        .checked_add(Set::<Id<Entry>>::worst_case(limits.connections)?.checked_mul(2)?)?
        .checked_add(each.checked_mul(u64::from(limits.connections))?)?
        .checked_add(crate::service::worst_case(&limits.connection)?)?
        .checked_add(Queue::<connection::Event>::worst_case(connection::MAX_UP)?)?
        .checked_add(Queue::<stream::Down>::worst_case(connection::MAX_DOWN)?)
}
