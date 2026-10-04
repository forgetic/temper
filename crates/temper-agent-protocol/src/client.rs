//! Named LLM calls and retained HTTP connections (llm.md §§4,8,10).
//!
//! Io names are component-local. A whole-agent loop sharing Io with channel,
//! files or process owners must map these tagged bindings to fresh global
//! names in a bounded router, retaining each until actual Closed. Loopback is
//! the only plaintext transport. Secured delegates TLS to an explicit owner;
//! no HTTP request is started before that owner confirms security.
//!
//! Drain `is_ready()` with `resume` before another input, reserve `MAX_UP` and
//! `MAX_OUT` in the output queues, and call `reclaim` at the reclaim point.
use crate::{Limits, exchange, grants, payload};
use alloc::boxed::Box;
use core::mem::size_of;
use core::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use skein_io as io;
use skein_lib::{Env, Id, Map, Queue, Set, Slab, Time, Token, Writer, stream};
use temper_agent_domain::{self as domain, llm};
use temper_channel::wire::{Address, EndpointDescriptor, Provider};

pub const MAX_UP: u32 = 1;
pub const MAX_OUT: u32 = exchange::MAX_DOWN + 3;
const IDENTITIES: u32 = 2;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Transport {
    Loopback,
    Secured,
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Endpoint {
    pub descriptor: EndpointDescriptor,
    pub transport: Transport,
}
#[expect(missing_debug_implementations, reason = "deployment metadata and HTTP headers remain protocol-owned")]
pub struct Identity {
    pub provider: Provider,
    pub value: exchange::Identity,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    Limits,
    Endpoint,
    Identity,
    Busy,
    Request,
}

#[expect(missing_debug_implementations, reason = "plaintext HTTP contains credential values")]
pub enum Security {
    Start { owner: Token, socket: Token, host: Box<[u8]> },
    Input { owner: Token, up: stream::Up },
    Output { owner: Token, down: stream::Down },
    Stop { owner: Token },
    Closed { owner: Token },
}
#[expect(missing_debug_implementations, reason = "HTTP stream effects contain credential values")]
pub enum Effect {
    Io(io::Request),
    Security(Security),
}
#[expect(missing_debug_implementations, reason = "plaintext stream events contain credential values")]
pub enum Event {
    Io(io::Event),
    Secured { owner: Token },
    Plain { owner: Token, up: stream::Up },
    Encrypted { owner: Token, down: stream::Down },
    SecurityFailed { owner: Token },
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    Connecting,
    Securing,
    Http,
    Closing,
    Closed,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CallState {
    Waiting,
    Running,
    Cancelled,
    Ended,
}
struct Call {
    owner: Token,
    endpoint: u32,
    prepared: Option<exchange::Exchange>,
    connection: Option<Id<Connection>>,
    state: CallState,
    absolute: Time,
}
struct Connection {
    endpoint: u32,
    transport: Transport,
    phase: Phase,
    socket: Option<Token>,
    call: Option<Id<Call>>,
    exchange: exchange::Exchange,
    progress: Time,
    security_started: bool,
}
#[expect(missing_debug_implementations, reason = "connections retain bearer/account bytes and opaque provider state")]
pub struct Client {
    endpoints: Box<[Endpoint]>,
    identities: Box<[Identity]>,
    salt: [u8; 16],
    sequence: u64,
    calls: Slab<Call>,
    connections: Slab<Connection>,
    owners: Map<Token, Id<Call>>,
    live: Set<Id<Connection>>,
    waiting: Set<Id<Call>>,
    ready: Set<Id<Connection>>,
    notices: Queue<domain::Event>,
    refused: Option<(Token, llm::Failure)>,
    upper: Queue<exchange::Event>,
    lower: Queue<stream::Down>,
}
impl Client {
    /// Salt is drawn once by startup from its injected randomness. Endpoint
    /// descriptors and identity data are frozen for this Client's lifetime.
    pub fn new(
        endpoints: Box<[Endpoint]>,
        identities: Box<[Identity]>,
        salt: [u8; 16],
        limits: &Limits,
        lower: &io::Limits,
    ) -> Result<Client, Error> {
        if !fits_io(limits, lower)
            || endpoints.len() > usize::try_from(limits.endpoints).expect("u32 fits usize")
            || identities.len() > usize::try_from(IDENTITIES).expect("two identities")
        {
            return Err(Error::Limits);
        }
        for (index, endpoint) in endpoints.iter().enumerate() {
            if payload::endpoints(core::slice::from_ref(&endpoint.descriptor), limits).is_err() {
                return Err(Error::Endpoint);
            }
            for other in endpoints.get(..index).ok_or(Error::Endpoint)? {
                if other.descriptor.endpoint == endpoint.descriptor.endpoint {
                    return Err(Error::Endpoint);
                }
            }
            if endpoint.transport == Transport::Loopback && !address(&endpoint.descriptor).ip().is_loopback() {
                return Err(Error::Endpoint);
            }
            let mut found = false;
            for identity in &identities {
                if identity.provider == endpoint.descriptor.provider {
                    found = true;
                }
            }
            if !found {
                return Err(Error::Identity);
            }
        }
        for (index, identity) in identities.iter().enumerate() {
            if exchange::identity_admit(&identity.value, limits).is_err() {
                return Err(Error::Identity);
            }
            for other in identities.get(..index).ok_or(Error::Identity)? {
                if other.provider == identity.provider {
                    return Err(Error::Identity);
                }
            }
        }
        Ok(Client {
            endpoints,
            identities,
            salt,
            sequence: 0,
            calls: Slab::with_capacity(limits.calls),
            connections: Slab::with_capacity(limits.calls),
            owners: Map::with_capacity(limits.calls),
            live: Set::with_capacity(limits.calls),
            waiting: Set::with_capacity(limits.calls),
            ready: Set::with_capacity(limits.calls),
            notices: Queue::with_capacity(2),
            refused: None,
            upper: Queue::with_capacity(exchange::MAX_UP),
            lower: Queue::with_capacity(exchange::MAX_DOWN),
        })
    }
    #[must_use]
    pub fn is_ready(&self) -> bool {
        !self.notices.is_empty()
            || self.refused.is_some()
            || !self.ready.is_empty()
            || (!self.waiting.is_empty() && self.can_assign())
    }
    #[must_use]
    pub fn takes(&self) -> bool {
        !self.is_ready()
    }
    #[must_use]
    pub fn bindings(&self) -> u32 {
        self.live.len()
    }
    #[must_use]
    pub fn connection(&self, owner: Token) -> Option<Token> {
        Some(self.calls.get(*self.owners.get(&owner)?)?.connection?.token())
    }
    #[must_use]
    pub fn next_deadline(&self, limits: &Limits) -> Option<Time> {
        let mut next = None;
        for &id in &self.waiting {
            if let Some(call) = self.calls.get(id) {
                next = Some(minimum(next, call.absolute));
            }
        }
        for &id in &self.live {
            if let Some(connection) = self.connections.get(id)
                && let Some(at) = deadline(connection, limits)
            {
                next = Some(minimum(next, at));
            }
        }
        next
    }
    #[must_use]
    pub fn is_due(&self, now: Time, limits: &Limits) -> bool {
        match self.next_deadline(limits) {
            Some(at) => at <= now,
            None => false,
        }
    }
    pub fn reclaim(&mut self) {
        self.calls.reclaim();
        self.connections.reclaim();
    }
    #[expect(clippy::manual_find, reason = "bounded concrete loop avoids closure-taking combinators")]
    fn endpoint(&self, name: u32) -> Option<&Endpoint> {
        for endpoint in &self.endpoints {
            if endpoint.descriptor.endpoint == name {
                return Some(endpoint);
            }
        }
        None
    }
    fn can_assign(&self) -> bool {
        if !self.connections.is_full() {
            return true;
        }
        for &id in &self.live {
            if let Some(connection) = self.connections.get(id)
                && connection.phase == Phase::Http
                && connection.call.is_none()
                && connection.exchange.is_idle()
                && !connection.exchange.has_work()
            {
                return true;
            }
        }
        false
    }
}

/// Admission owns and measures the prompt once, before touching a connection.
/// Domain terminals are retained for the next up/resume pass.
pub fn down(
    client: &mut Client,
    env: &Env<Limits>,
    table: &grants::Table,
    request: domain::Request,
    effects: &mut Queue<Effect>,
) -> Result<(), Error> {
    if !client.takes() {
        return Err(Error::Busy);
    }
    match request {
        domain::Request::Complete { owner, grant, prompt, timeout } => {
            if client.owners.get(&owner).is_some() {
                return Err(Error::Busy);
            }
            if client.calls.is_full() {
                client.refused = Some((owner, llm::Failure::Unavailable));
                return Ok(());
            }
            let Some(sequence) = client.sequence.checked_add(1) else {
                client.refused = Some((owner, llm::Failure::Invalid));
                return Ok(());
            };
            client.sequence = sequence;
            let endpoint_name = prompt.endpoint.0;
            let Some(endpoint) = client.endpoint(endpoint_name) else {
                client.refused = Some((owner, llm::Failure::Invalid));
                return Ok(());
            };
            let mut selected = None;
            for identity in &client.identities {
                if identity.provider == endpoint.descriptor.provider {
                    selected = Some(&identity.value);
                }
            }
            let Some(identity) = selected else {
                client.refused = Some((owner, llm::Failure::Invalid));
                return Ok(());
            };
            let admission = exchange::Admission {
                owner,
                grant,
                prompt,
                timeout,
                session: uuid(client.salt, owner.raw(), false),
                request: uuid(client.salt, sequence, true),
            };
            let prepared = match exchange::Exchange::prepare(
                admission,
                &endpoint.descriptor,
                table,
                identity,
                env.now,
                &env.limits,
            ) {
                Ok(prepared) => prepared,
                Err(failure) => {
                    client.refused = Some((owner, failure));
                    return Ok(());
                }
            };
            let absolute = prepared.absolute();
            let Ok(id) = client.calls.insert(Call {
                owner,
                endpoint: endpoint_name,
                prepared: Some(prepared),
                connection: None,
                state: CallState::Waiting,
                absolute,
            }) else {
                unreachable!("checked call capacity");
            };
            assert!(client.owners.insert(owner, id).is_ok(), "one owner per live call");
            assert!(client.waiting.insert(id).is_ok(), "waiting is bounded by calls");
            assign(client, env, effects);
            Ok(())
        }
        domain::Request::Cancel { owner } => {
            cancel(client, env, owner, effects);
            Ok(())
        }
        domain::Request::Admitted { .. }
        | domain::Request::Answer { .. }
        | domain::Request::Checking { .. }
        | domain::Request::Push { .. }
        | domain::Request::CancelHost { .. }
        | domain::Request::Rejected { .. }
        | domain::Request::Exhausted { .. }
        | domain::Request::Io { .. }
        | domain::Request::CancelIo { .. }
        | domain::Request::Read { .. }
        | domain::Request::Probe { .. }
        | domain::Request::Check { .. }
        | domain::Request::Abort { .. } => Err(Error::Request),
    }
}
fn assign(client: &mut Client, env: &Env<Limits>, effects: &mut Queue<Effect>) {
    let Some(&call_id) = client.waiting.first() else {
        return;
    };
    let Some(call) = client.calls.get(call_id) else {
        return;
    };
    let endpoint = call.endpoint;
    let mut kept = None;
    let mut oldest = None;
    for &id in &client.live {
        let Some(connection) = client.connections.get(id) else {
            continue;
        };
        if connection.phase != Phase::Http
            || connection.call.is_some()
            || !connection.exchange.is_idle()
            || connection.exchange.has_work()
        {
            continue;
        }
        if connection.endpoint == endpoint {
            kept = Some(id);
            break;
        }
        match oldest {
            Some((_, time)) if time <= connection.progress => {}
            Some(_) | None => oldest = Some((id, connection.progress)),
        }
    }
    if let Some(id) = kept {
        let prepared =
            client.calls.get_mut(call_id).expect("waiting call").prepared.take().expect("prepared waiting call");
        let connection = client.connections.get_mut(id).expect("kept connection");
        match connection.exchange.next(prepared) {
            Ok(()) => {}
            Err(prepared) => {
                client.calls.get_mut(call_id).expect("waiting call").prepared = Some(prepared);
                return;
            }
        }
        connection.call = Some(call_id);
        connection.progress = env.now;
        client.waiting.remove(&call_id);
        let call = client.calls.get_mut(call_id).expect("waiting call");
        call.connection = Some(id);
        call.state = CallState::Running;
        exchange::start(&mut connection.exchange, env, &mut client.upper, &mut client.lower);
        collect(client, id, env, effects);
        return;
    }
    if client.connections.is_full() {
        if let Some((id, _)) = oldest {
            let connection = client.connections.get_mut(id).expect("oldest live connection");
            exchange::close(&mut connection.exchange, env, &mut client.upper, &mut client.lower);
            collect(client, id, env, effects);
        }
        return;
    }
    let route = client.endpoint(endpoint).expect("admitted endpoint");
    let transport = route.transport;
    let addr = address(&route.descriptor);
    let prepared = client.calls.get_mut(call_id).expect("waiting call").prepared.take().expect("prepared waiting call");
    let Ok(id) = client.connections.insert(Connection {
        endpoint,
        transport,
        phase: Phase::Connecting,
        socket: None,
        call: Some(call_id),
        exchange: prepared,
        progress: env.now,
        security_started: false,
    }) else {
        unreachable!("checked connection capacity");
    };
    assert!(client.live.insert(id).is_ok(), "connections bound by their slab");
    client.waiting.remove(&call_id);
    let call = client.calls.get_mut(call_id).expect("waiting call");
    call.connection = Some(id);
    call.state = CallState::Running;
    effects.push(Effect::Io(io::Request::Connect { owner: id.token(), addr }));
}
fn cancel(client: &mut Client, env: &Env<Limits>, owner: Token, effects: &mut Queue<Effect>) {
    let Some(&id) = client.owners.get(&owner) else {
        return;
    };
    let call = client.calls.get_mut(id).expect("owned call");
    match call.state {
        CallState::Waiting => {
            call.prepared = None;
            call.state = CallState::Ended;
            client.waiting.remove(&id);
            client.owners.remove(&owner);
            client.calls.retire(id);
            client.notices.push(domain::Event::Cancelled { owner });
        }
        CallState::Running => {
            call.state = CallState::Cancelled;
            let connection = call.connection.expect("running binding");
            let bound = client.connections.get_mut(connection).expect("running connection");
            exchange::cancel(&mut bound.exchange, env, &mut client.upper, &mut client.lower);
            collect(client, connection, env, effects);
        }
        CallState::Cancelled | CallState::Ended => {}
    }
}

pub fn up(
    client: &mut Client,
    env: &Env<Limits>,
    event: Event,
    above: &mut Queue<domain::Event>,
    effects: &mut Queue<Effect>,
) {
    match event {
        Event::Io(event) => io_event(client, env, event, effects),
        Event::Secured { owner } => {
            let id = Id::<Connection>::from_token(owner);
            if client.live.contains(&id)
                && let Some(connection) = client.connections.get_mut(id)
                && connection.phase == Phase::Securing
            {
                connection.phase = Phase::Http;
                connection.progress = env.now;
                exchange::start(&mut connection.exchange, env, &mut client.upper, &mut client.lower);
                collect(client, id, env, effects);
            }
        }
        Event::Plain { owner, up } => {
            let id = Id::<Connection>::from_token(owner);
            if client.live.contains(&id)
                && let Some(connection) = client.connections.get_mut(id)
                && connection.phase == Phase::Http
                && connection.transport == Transport::Secured
            {
                exchange::up(&mut connection.exchange, env, up, &mut client.upper, &mut client.lower);
                collect(client, id, env, effects);
            }
        }
        Event::Encrypted { owner, down } => {
            let id = Id::<Connection>::from_token(owner);
            if client.live.contains(&id)
                && let Some(connection) = client.connections.get(id)
                && connection.transport == Transport::Secured
            {
                match connection.phase {
                    Phase::Securing | Phase::Http => {
                        if let Some(socket) = connection.socket {
                            effects.push(Effect::Io(io::Request::Stream { stream: socket, down }));
                        }
                    }
                    Phase::Connecting | Phase::Closing | Phase::Closed => {}
                }
            }
        }
        Event::SecurityFailed { owner } => {
            let id = Id::<Connection>::from_token(owner);
            if client.live.contains(&id)
                && let Some(connection) = client.connections.get_mut(id)
            {
                match connection.phase {
                    Phase::Securing | Phase::Http => {
                        exchange::transport_failed(&mut connection.exchange, env, &mut client.upper, &mut client.lower);
                        collect(client, id, env, effects);
                    }
                    Phase::Connecting | Phase::Closing | Phase::Closed => {}
                }
            }
        }
    }
    publish(client, above);
}
fn io_event(client: &mut Client, env: &Env<Limits>, event: io::Event, effects: &mut Queue<Effect>) {
    match event {
        io::Event::Connecting { owner, socket } => {
            let id = Id::<Connection>::from_token(owner);
            if client.live.contains(&id)
                && let Some(connection) = client.connections.get_mut(id)
                && connection.socket.is_none()
            {
                connection.socket = Some(socket);
                if connection.phase == Phase::Closing {
                    effects.push(Effect::Io(io::Request::Abort { entity: socket }));
                }
            }
        }
        io::Event::Connected { owner } => {
            let id = Id::<Connection>::from_token(owner);
            if !client.live.contains(&id) {
                return;
            }
            let Some(connection) = client.connections.get(id) else {
                return;
            };
            if connection.phase != Phase::Connecting {
                return;
            }
            let Some(socket) = connection.socket else {
                return;
            };
            let endpoint = connection.endpoint;
            let transport = connection.transport;
            let host = match transport {
                Transport::Loopback => None,
                Transport::Secured => {
                    Some(client.endpoint(endpoint).expect("connection endpoint").descriptor.host.clone())
                }
            };
            let connection = client.connections.get_mut(id).expect("live connection");
            connection.progress = env.now;
            match host {
                Some(host) => {
                    connection.phase = Phase::Securing;
                    connection.security_started = true;
                    effects.push(Effect::Security(Security::Start { owner, socket, host }));
                }
                None => {
                    connection.phase = Phase::Http;
                    exchange::start(&mut connection.exchange, env, &mut client.upper, &mut client.lower);
                    collect(client, id, env, effects);
                }
            }
        }
        io::Event::Stream { owner, up } => {
            let id = Id::<Connection>::from_token(owner);
            if !client.live.contains(&id) {
                return;
            }
            let Some(connection) = client.connections.get_mut(id) else {
                return;
            };
            match connection.phase {
                Phase::Securing | Phase::Http => match connection.transport {
                    Transport::Secured => effects.push(Effect::Security(Security::Input { owner, up })),
                    Transport::Loopback => {
                        exchange::up(&mut connection.exchange, env, up, &mut client.upper, &mut client.lower);
                        collect(client, id, env, effects);
                    }
                },
                Phase::Connecting | Phase::Closing | Phase::Closed => {}
            }
        }
        io::Event::Failed { owner, error: _ } => {
            let id = Id::<Connection>::from_token(owner);
            if client.live.contains(&id)
                && let Some(connection) = client.connections.get_mut(id)
                && connection.phase != Phase::Closed
                && connection.phase != Phase::Closing
            {
                exchange::transport_failed(&mut connection.exchange, env, &mut client.upper, &mut client.lower);
                collect(client, id, env, effects);
            }
        }
        io::Event::Closed { owner } => {
            let id = Id::<Connection>::from_token(owner);
            if !client.live.remove(&id) {
                return;
            }
            let connection = client.connections.get_mut(id).expect("closed live binding");
            connection.phase = Phase::Closed;
            exchange::closed(&mut connection.exchange, env, &mut client.upper, &mut client.lower);
            if connection.security_started {
                effects.push(Effect::Security(Security::Closed { owner }));
            }
            collect(client, id, env, effects);
            client.ready.remove(&id);
            client.connections.retire(id);
        }
        io::Event::Listening { .. } | io::Event::Accepted { .. } => {}
    }
}

pub fn resume(client: &mut Client, env: &Env<Limits>, above: &mut Queue<domain::Event>, effects: &mut Queue<Effect>) {
    if publish(client, above) {
        return;
    }
    if let Some((owner, failure)) = client.refused.take() {
        above.push(domain::Event::Failed { owner, failure });
        return;
    }
    if let Some(id) = client.ready.pop_first() {
        if client.live.contains(&id)
            && let Some(connection) = client.connections.get_mut(id)
        {
            exchange::resume(&mut connection.exchange, env, &mut client.upper, &mut client.lower);
            collect(client, id, env, effects);
            publish(client, above);
        }
    } else {
        assign(client, env, effects);
        publish(client, above);
    }
}
pub fn fire(client: &mut Client, env: &Env<Limits>, above: &mut Queue<domain::Event>, effects: &mut Queue<Effect>) {
    let mut expired = None;
    for &id in &client.waiting {
        if let Some(call) = client.calls.get(id)
            && call.absolute <= env.now
        {
            expired = Some(id);
            break;
        }
    }
    if let Some(id) = expired {
        terminal(
            client,
            id,
            domain::Event::Failed {
                owner: client.calls.get(id).expect("waiting call").owner,
                failure: llm::Failure::TimedOut,
            },
        );
        client.waiting.remove(&id);
        publish(client, above);
        return;
    }
    let mut due = None;
    for &id in &client.live {
        if let Some(connection) = client.connections.get(id)
            && let Some(at) = deadline(connection, &env.limits)
            && at <= env.now
        {
            due = Some(id);
            break;
        }
    }
    if let Some(id) = due {
        let connection = client.connections.get_mut(id).expect("due connection");
        match connection.phase {
            Phase::Connecting | Phase::Securing => {
                if connection.exchange.absolute() <= env.now {
                    exchange::fire(&mut connection.exchange, env, &mut client.upper, &mut client.lower);
                } else {
                    exchange::transport_failed(&mut connection.exchange, env, &mut client.upper, &mut client.lower);
                }
            }
            Phase::Http => exchange::fire(&mut connection.exchange, env, &mut client.upper, &mut client.lower),
            Phase::Closing | Phase::Closed => {}
        }
        collect(client, id, env, effects);
        publish(client, above);
    }
}
fn collect(client: &mut Client, id: Id<Connection>, env: &Env<Limits>, effects: &mut Queue<Effect>) {
    let Some(connection) = client.connections.get(id) else {
        return;
    };
    let socket = connection.socket;
    let transport = connection.transport;
    let phase = connection.phase;
    for _slot in 0..exchange::MAX_DOWN {
        let Some(down) = client.lower.pop() else {
            break;
        };
        match phase {
            Phase::Connecting | Phase::Closing | Phase::Closed => {}
            Phase::Securing | Phase::Http => match transport {
                Transport::Loopback => {
                    if let Some(socket) = socket {
                        effects.push(Effect::Io(io::Request::Stream { stream: socket, down }));
                    }
                }
                Transport::Secured => effects.push(Effect::Security(Security::Output { owner: id.token(), down })),
            },
        }
    }
    for _slot in 0..exchange::MAX_UP {
        let Some(event) = client.upper.pop() else {
            break;
        };
        match event {
            exchange::Event::Completed { owner, completion } => {
                if let Some(call) = client.connections.get(id).expect("routed connection").call {
                    terminal(client, call, domain::Event::Completed { owner, completion });
                    client.connections.get_mut(id).expect("routed connection").call = None;
                }
            }
            exchange::Event::Failed { owner, failure, evidence: _, detail: _ } => {
                if let Some(call) = client.connections.get(id).expect("routed connection").call {
                    terminal(client, call, domain::Event::Failed { owner, failure });
                    client.connections.get_mut(id).expect("routed connection").call = None;
                }
            }
            exchange::Event::Closed { owner, cancelled } => {
                if cancelled && let Some(call) = client.connections.get(id).expect("routed connection").call {
                    terminal(client, call, domain::Event::Cancelled { owner });
                    client.connections.get_mut(id).expect("routed connection").call = None;
                }
            }
            exchange::Event::Close => abort(client, id, effects),
            exchange::Event::Idle => client.connections.get_mut(id).expect("routed connection").progress = env.now,
        }
    }
    if let Some(connection) = client.connections.get(id)
        && connection.phase != Phase::Closed
        && connection.exchange.has_work()
    {
        assert!(client.ready.insert(id).is_ok(), "ready bounded by live connections");
    }
}
fn abort(client: &mut Client, id: Id<Connection>, effects: &mut Queue<Effect>) {
    let connection = client.connections.get_mut(id).expect("routed connection");
    if connection.phase == Phase::Closing || connection.phase == Phase::Closed {
        return;
    }
    connection.phase = Phase::Closing;
    if connection.security_started {
        effects.push(Effect::Security(Security::Stop { owner: id.token() }));
    }
    if let Some(socket) = connection.socket {
        effects.push(Effect::Io(io::Request::Abort { entity: socket }));
    }
}
fn terminal(client: &mut Client, id: Id<Call>, event: domain::Event) {
    let Some(call) = client.calls.get_mut(id) else {
        return;
    };
    if call.state == CallState::Ended {
        return;
    }
    call.state = CallState::Ended;
    call.prepared = None;
    client.owners.remove(&call.owner);
    client.calls.retire(id);
    client.notices.push(event);
}
fn publish(client: &mut Client, above: &mut Queue<domain::Event>) -> bool {
    if let Some(event) = client.notices.pop() {
        above.push(event);
        true
    } else {
        false
    }
}
fn minimum(previous: Option<Time>, at: Time) -> Time {
    match previous {
        Some(previous) => previous.min(at),
        None => at,
    }
}
fn deadline(connection: &Connection, limits: &Limits) -> Option<Time> {
    match connection.phase {
        Phase::Connecting => {
            Some(connection.progress.saturating_add(limits.connect).min(connection.exchange.absolute()))
        }
        Phase::Securing => {
            Some(connection.progress.saturating_add(limits.handshake).min(connection.exchange.absolute()))
        }
        Phase::Http => connection.exchange.deadline(limits),
        Phase::Closing | Phase::Closed => None,
    }
}
fn address(endpoint: &EndpointDescriptor) -> SocketAddr {
    match endpoint.address {
        Address::V4 { bytes } => SocketAddr::from((Ipv4Addr::from(bytes), endpoint.port)),
        Address::V6 { bytes } => SocketAddr::from((Ipv6Addr::from(bytes), endpoint.port)),
    }
}
fn uuid(mut salt: [u8; 16], name: u64, request: bool) -> Box<[u8]> {
    if request {
        salt[0] ^= 0x80;
    }
    for (index, byte) in name.to_be_bytes().iter().enumerate() {
        let at = if index == 0 { 7 } else { index.saturating_add(8) };
        *salt.get_mut(at).expect("eight token bytes in UUID positions") ^= *byte;
    }
    salt[6] = (salt[6] & 0x0f) | 0x40;
    salt[8] = (salt[8] & 0x3f) | 0x80;
    let hex = b"0123456789abcdef";
    let mut out = Writer::new(36);
    for (index, byte) in salt.iter().enumerate() {
        if index == 4 || index == 6 || index == 8 || index == 10 {
            out.put(b"-").expect("UUID separator");
        }
        out.put(&[
            *hex.get(usize::from(byte >> 4_u32)).expect("high nibble"),
            *hex.get(usize::from(byte & 15)).expect("low nibble"),
        ])
        .expect("UUID digits");
    }
    out.finish()
}
#[must_use]
pub fn fits_io(limits: &Limits, lower: &io::Limits) -> bool {
    let records = match u64::from(limits.request_bytes).checked_add(u64::from(limits.chunk).saturating_sub(1)) {
        Some(bytes) if limits.chunk > 0 => bytes.div_euclid(u64::from(limits.chunk)).checked_add(1),
        Some(_) | None => None,
    };
    let records_fit = match records {
        Some(records) => records <= u64::from(lower.sends).saturating_add(1),
        None => false,
    };
    worst_case(limits).is_some()
        && io::worst_case(lower).is_some()
        && skein_http::client::largest_read(&exchange::http_limits(limits)) <= lower.intake
        && skein_http::client::largest_room(&exchange::http_limits(limits)) <= lower.output
        && lower.sends > 0
        && records_fit
}
/// The whole owner counts simultaneous pending+active attempts, prompt and
/// document/body encoding overlap, descriptors/identity data, routing queues,
/// and a completion waiting for the domain. Its loop adds Io's own bound and
/// the external security owner's separately audited bound.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    if limits.calls == 0 {
        return None;
    }
    let descriptor =
        u64::try_from(size_of::<Endpoint>()).ok()?.checked_add(u64::from(limits.name_bytes).checked_mul(3)?)?;
    let identity = u64::try_from(size_of::<Identity>())
        .ok()?
        .checked_add(u64::from(limits.headers).checked_mul(u64::try_from(size_of::<skein_http::Header>()).ok()?)?)?
        .checked_add(u64::from(limits.head_bytes))?
        .checked_add(u64::from(limits.request_bytes).checked_mul(3)?)?
        .checked_add(u64::from(limits.parts).checked_mul(u64::try_from(size_of::<Box<[u8]>>()).ok()?)?)?
        .checked_add(temper_llm_anthropic::worst_case(&limits.anthropic())?.checked_mul(2)?)?;
    u64::try_from(size_of::<Client>())
        .ok()?
        .checked_add(Slab::<Call>::worst_case(limits.calls)?)?
        .checked_add(Slab::<Connection>::worst_case(limits.calls)?)?
        .checked_add(Map::<Token, Id<Call>>::worst_case(limits.calls)?)?
        .checked_add(Set::<Id<Call>>::worst_case(limits.calls)?)?
        .checked_add(Set::<Id<Connection>>::worst_case(limits.calls)?.checked_mul(2)?)?
        .checked_add(exchange::worst_case(limits)?.checked_mul(u64::from(limits.calls).checked_mul(2)?)?)?
        .checked_add(descriptor.checked_mul(u64::from(limits.endpoints))?)?
        .checked_add(identity.checked_mul(u64::from(IDENTITIES))?)?
        .checked_add(u64::from(limits.answer_bytes).checked_mul(2)?)?
        .checked_add(Queue::<domain::Event>::worst_case(2)?)?
        .checked_add(Queue::<exchange::Event>::worst_case(exchange::MAX_UP)?)?
        .checked_add(Queue::<stream::Down>::worst_case(exchange::MAX_DOWN)?)
}
