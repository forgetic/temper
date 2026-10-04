//! Engine listener and authenticated links. TLS is an explicit owner boundary;
//! this module never interprets encrypted socket bytes as channel frames.
use alloc::boxed::Box;
use skein_io::{self as io, kernel::Addr};
use skein_lib::{Deadlines, Env, Id, Map, Queue, Set, Slab, Time, Token, stream};
use subtle::ConstantTimeEq;
use temper_channel::{Sizes, machine, wire};
use temper_engine_domain::{self as engine, Request};

use crate::{
    Limits,
    connection::{Connection, Phase, Transport},
    translate::{self, Repository, Value},
};

/// Three channel operations, each bounded by `MAX_DOWN`, plus bind/close effects.
pub const MAX_OUT: u32 = 16;
/// An old link's Lost can precede the new link's notification.
pub const MAX_UP: u32 = 3;
/// Slab slot indices never reach `u32::MAX`, even at maximum capacity.
pub const OWNER: Token = Token::new(u64::MAX);

#[expect(missing_debug_implementations, reason = "authentication secrets must never occur in traces")]
pub struct Worker {
    pub name: Box<[u8]>,
    pub secret: Box<[u8]>,
}

#[derive(Debug)]
pub enum Notice {
    Domain(engine::Event),
    Listening { addr: Addr },
    Failed { error: io::Error },
    Closed,
}

/// The TLS owner must retain its own binding until actual io Closed. Output
/// Finish flushes its records then closes io; it never reports plaintext ready
/// until the authenticated handshake succeeds. There is no TLS implementation here.
#[derive(Debug)]
pub enum Security {
    Start { owner: Token, socket: Token, peer: Addr },
    Input { owner: Token, up: stream::Up },
    Output { owner: Token, down: stream::Down },
    Stop { owner: Token },
    Closed { owner: Token },
}

#[derive(Debug)]
pub enum Effect {
    Io(io::Request),
    Security(Security),
}

#[derive(Debug)]
pub enum Event {
    Io(io::Event),
    Secured { owner: Token },
    Plain { owner: Token, up: stream::Up },
    SecurityFailed { owner: Token },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Idle,
    Starting,
    Listening,
    Closing { listener_closed: bool },
    Closed,
}

#[expect(
    missing_debug_implementations,
    reason = "configured secrets and queued grant values must never occur in traces"
)]
pub struct Listener {
    workers: Box<[Worker]>,
    repositories: Box<[Repository]>,
    transport: Transport,
    state: State,
    handle: Option<Token>,
    connections: Slab<Connection>,
    bound: Set<Id<Connection>>,
    current: Map<u32, Id<Connection>>,
    pending: Map<u32, Id<Connection>>,
    ready: Set<Id<Connection>>,
    alarms: Deadlines<Id<Connection>>,
    held: u32,
}

impl Listener {
    /// Conservatively validate plaintext channel capacity for both transports.
    /// The external TLS owner must additionally validate its encrypted reads,
    /// records and output against these actual io limits before using Secured.
    #[must_use]
    pub fn new(
        workers: Box<[Worker]>,
        repositories: Box<[Repository]>,
        transport: Transport,
        limits: &Limits,
        sizes: &Sizes,
        io_limits: &io::Limits,
    ) -> Option<Listener> {
        if !limits.fits_io(sizes, io_limits)
            || u32::try_from(workers.len()).ok()? > limits.workers
            || u32::try_from(repositories.len()).ok()? > limits.repositories
        {
            return None;
        }
        for (index, worker) in workers.iter().enumerate() {
            if worker.name.is_empty()
                || worker.name.len() > usize::try_from(limits.channel.name_bytes).ok()?
                || worker.secret.is_empty()
                || worker.secret.len() > usize::try_from(limits.channel.secret_bytes).ok()?
            {
                return None;
            }
            for prior in workers.get(..index)? {
                if prior.name == worker.name {
                    return None;
                }
            }
        }
        for repository in &repositories {
            if repository.name.len() > usize::try_from(sizes.name_bytes).ok()?
                || repository.remote.len() > usize::try_from(sizes.name_bytes).ok()?
            {
                return None;
            }
        }
        Some(Listener {
            workers,
            repositories,
            transport,
            state: State::Idle,
            handle: None,
            connections: Slab::with_capacity(limits.connections),
            bound: Set::with_capacity(limits.connections),
            current: Map::with_capacity(limits.workers),
            pending: Map::with_capacity(limits.workers),
            ready: Set::with_capacity(limits.connections),
            alarms: Deadlines::with_capacity(limits.connections),
            held: 0,
        })
    }

    #[must_use]
    pub fn is_ready(&self) -> bool {
        if !self.ready.is_empty() {
            return true;
        }
        match self.state {
            State::Closing { listener_closed } => {
                if listener_closed && self.held == 0 {
                    return true;
                }
                for &id in &self.bound {
                    let entry = self.connections.get(id).expect("a bound connection exists");
                    if entry.phase != Phase::Closing && entry.phase != Phase::Closed {
                        return true;
                    }
                }
            }
            State::Idle | State::Starting | State::Listening | State::Closed => {}
        }
        false
    }
    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        self.alarms.next()
    }
    #[must_use]
    pub fn is_due(&self, now: Time) -> bool {
        match self.alarms.next() {
            Some(at) => at <= now,
            None => false,
        }
    }
    #[must_use]
    pub const fn connections(&self) -> u32 {
        self.held
    }
    #[must_use]
    pub fn phase(&self, channel: Token) -> Option<Phase> {
        Some(self.connections.get(Id::from_token(channel))?.phase)
    }
    #[must_use]
    pub fn channel(&self, worker: u32) -> Option<Token> {
        Some(self.current.get(&worker)?.token())
    }
    #[must_use]
    pub const fn is_closed(&self) -> bool {
        matches_state_closed(self.state)
    }
    pub fn reclaim(&mut self) {
        self.connections.reclaim();
    }
}

const fn matches_state_closed(state: State) -> bool {
    match state {
        State::Closed => true,
        State::Idle | State::Starting | State::Listening | State::Closing { .. } => false,
    }
}

/// Only explicitly configured loopback plaintext is accepted without TLS.
#[must_use]
pub fn start(listener: &mut Listener, addr: Addr, effects: &mut Queue<Effect>) -> bool {
    if listener.state != State::Idle || (listener.transport == Transport::Loopback && !addr.ip().is_loopback()) {
        return false;
    }
    listener.state = State::Starting;
    effects.push(Effect::Io(io::Request::Listen { owner: OWNER, addr }));
    true
}

pub fn close(listener: &mut Listener, effects: &mut Queue<Effect>) {
    match listener.state {
        State::Idle => listener.state = State::Closing { listener_closed: true },
        State::Starting => listener.state = State::Closing { listener_closed: false },
        State::Listening => {
            listener.state = State::Closing { listener_closed: false };
            effects.push(Effect::Io(io::Request::Close { entity: listener.handle.expect("listening has a handle") }));
        }
        State::Closing { .. } | State::Closed => {}
    }
}

pub fn up(
    listener: &mut Listener,
    env: &Env<Limits>,
    sizes: &Sizes,
    event: Event,
    out: &mut Queue<Notice>,
    effects: &mut Queue<Effect>,
) {
    match event {
        Event::Io(event) => io_up(listener, env, sizes, event, out, effects),
        Event::Secured { owner } => {
            let id = Id::from_token(owner);
            let Some(entry) = listener.connections.get_mut(id) else {
                return;
            };
            if entry.phase != Phase::Securing || entry.transport != Transport::Secured {
                return;
            }
            entry.phase = Phase::Opening;
            entry.poll(&env.limits, sizes);
            drain(listener, env, sizes, id, out, effects);
        }
        Event::Plain { owner, up } => {
            let id = Id::from_token(owner);
            let Some(entry) = listener.connections.get_mut(id) else {
                return;
            };
            if entry.transport != Transport::Secured || entry.phase == Phase::Securing || entry.phase == Phase::Closed {
                return;
            }
            entry.input(&env.limits, sizes, env.now, up);
            drain(listener, env, sizes, id, out, effects);
        }
        Event::SecurityFailed { owner } => abort(listener, env, sizes, Id::from_token(owner), effects),
    }
}

fn io_up(
    listener: &mut Listener,
    env: &Env<Limits>,
    sizes: &Sizes,
    event: io::Event,
    out: &mut Queue<Notice>,
    effects: &mut Queue<Effect>,
) {
    match event {
        io::Event::Listening { owner, listener: handle, addr } if owner == OWNER => {
            listener.handle = Some(handle);
            if listener.state == State::Starting {
                listener.state = State::Listening;
                out.push(Notice::Listening { addr });
            } else {
                effects.push(Effect::Io(io::Request::Close { entity: handle }));
            }
        }
        io::Event::Accepted { owner, socket, peer } if owner == OWNER => {
            accepted(listener, env, sizes, socket, peer, effects);
        }
        io::Event::Stream { owner, up } => {
            let id = Id::from_token(owner);
            let Some(entry) = listener.connections.get_mut(id) else {
                return;
            };
            if entry.phase == Phase::Closed {
                return;
            }
            match entry.transport {
                Transport::Loopback => {
                    entry.input(&env.limits, sizes, env.now, up);
                    drain(listener, env, sizes, id, out, effects);
                }
                Transport::Secured => effects.push(Effect::Security(Security::Input { owner, up })),
            }
        }
        io::Event::Closed { owner } if owner == OWNER => {
            listener.handle = None;
            listener.state = State::Closing { listener_closed: true };
        }
        io::Event::Closed { owner } => closed(listener, env, sizes, Id::from_token(owner), out, effects),
        io::Event::Failed { owner, error } if owner == OWNER => {
            out.push(Notice::Failed { error });
            close(listener, effects);
        }
        io::Event::Failed { owner, error: _ } => abort(listener, env, sizes, Id::from_token(owner), effects),
        io::Event::Listening { .. }
        | io::Event::Accepted { .. }
        | io::Event::Connecting { .. }
        | io::Event::Connected { .. } => {}
    }
}

fn accepted(
    listener: &mut Listener,
    env: &Env<Limits>,
    sizes: &Sizes,
    socket: Token,
    peer: Addr,
    effects: &mut Queue<Effect>,
) {
    if listener.state != State::Listening
        || listener.connections.is_full()
        || (listener.transport == Transport::Loopback && !peer.ip().is_loopback())
    {
        effects.push(Effect::Io(io::Request::Reject { socket }));
        return;
    }
    let entry = Connection::new(socket, listener.transport, env.now, &env.limits, sizes)
        .expect("startup bounds validated machines");
    let id = listener.connections.insert(entry).expect("entrance has a free slot");
    let fresh = listener.bound.insert(id).expect("one mark per bound connection");
    assert!(fresh, "the new binding is distinct");
    listener.held = listener.held.checked_add(1).expect("configured connection count fits");
    effects.push(Effect::Io(io::Request::Bind { socket, owner: id.token() }));
    match listener.transport {
        Transport::Loopback => {
            listener.ready.insert(id).expect("one ready mark per connection");
        }
        Transport::Secured => effects.push(Effect::Security(Security::Start { owner: id.token(), socket, peer })),
    }
    follow(listener, env, id);
}

fn closed(
    listener: &mut Listener,
    env: &Env<Limits>,
    sizes: &Sizes,
    id: Id<Connection>,
    out: &mut Queue<Notice>,
    effects: &mut Queue<Effect>,
) {
    let Some(entry) = listener.connections.get_mut(id) else {
        return;
    };
    if entry.phase == Phase::Closed {
        return;
    }
    entry.phase = Phase::Closed;
    let worker = entry.worker;
    if entry.announced {
        entry.announced = false;
        out.push(Notice::Domain(engine::Event::Lost { channel: id.token() }));
    }
    if entry.transport == Transport::Secured {
        effects.push(Effect::Security(Security::Closed { owner: id.token() }));
    }
    listener.ready.remove(&id);
    listener.alarms.cancel(id);
    listener.connections.retire(id);
    let removed = listener.bound.remove(&id);
    assert!(removed, "actual Closed releases the bound connection");
    listener.held = listener.held.checked_sub(1).expect("an actual Closed releases one binding");
    if let Some(worker) = worker {
        released(listener, env, sizes, worker, id, effects);
    }
}

fn released(
    listener: &mut Listener,
    env: &Env<Limits>,
    sizes: &Sizes,
    worker: u32,
    id: Id<Connection>,
    effects: &mut Queue<Effect>,
) {
    if listener.pending.get(&worker) == Some(&id) {
        let removed = listener.pending.remove(&worker);
        assert!(removed == Some(id), "the closed pending connection is removed");
    }
    if listener.current.get(&worker) != Some(&id) {
        return;
    }
    let removed = listener.current.remove(&worker);
    assert!(removed == Some(id), "current binding is the closed connection");
    let Some(next) = listener.pending.remove(&worker) else {
        return;
    };
    let entry = listener.connections.get(next).expect("a pending replacement stays bound");
    if listener.state == State::Listening && entry.phase == Phase::Pending && entry.until > env.now {
        // Lost was emitted only after old io Closed, before the new Accept.
        activate(listener, env, sizes, worker, next, effects);
    }
}

/// Domain output never waits behind a peer; a full machine closes that peer.
pub fn down(
    listener: &mut Listener,
    env: &Env<Limits>,
    sizes: &Sizes,
    request: Request,
    values: &[Value],
    effects: &mut Queue<Effect>,
) {
    let channel = match &request {
        Request::Assign { channel, .. }
        | Request::Inbound { channel, .. }
        | Request::Cancel { channel, .. }
        | Request::Relayed { channel, .. }
        | Request::Acknowledge { channel, .. }
        | Request::Grant { channel, .. }
        | Request::Refuse { channel } => *channel,
        Request::Account { .. }
        | Request::Forge { .. }
        | Request::Reply { .. }
        | Request::Deliver { .. }
        | Request::Ended { .. }
        | Request::Store { .. } => return,
    };
    let id = Id::from_token(channel);
    let Some(entry) = listener.connections.get(id) else {
        return;
    };
    if entry.phase != Phase::Open {
        return;
    }
    match translate::down(request, &listener.repositories, values, env.now, sizes) {
        Ok(Some((_, wire::Message::Refuse { refuse }))) => finish(listener, env, sizes, id, Some(refuse), effects),
        Ok(Some((_, message))) => {
            let entry = listener.connections.get_mut(id).expect("the connection remains bound");
            entry.request(&env.limits, sizes, machine::Request::Send(message));
            discard(listener, env, id, effects);
        }
        Ok(None) => {}
        Err(
            translate::Error::Name
            | translate::Error::Repository
            | translate::Error::Credential
            | translate::Error::Payload
            | translate::Error::Direction
            | translate::Error::InvalidRelay { .. },
        ) => finish(listener, env, sizes, id, None, effects),
    }
}

pub fn resume(
    listener: &mut Listener,
    env: &Env<Limits>,
    sizes: &Sizes,
    out: &mut Queue<Notice>,
    effects: &mut Queue<Effect>,
) {
    if let Some(&id) = listener.ready.first() {
        listener.ready.remove(&id);
        let entry = listener.connections.get_mut(id).expect("ready connections stay bound");
        entry.poll(&env.limits, sizes);
        return drain(listener, env, sizes, id, out, effects);
    }
    match listener.state {
        State::Closing { listener_closed } => {
            if listener_closed && listener.held == 0 {
                listener.state = State::Closed;
                out.push(Notice::Closed);
                return;
            }
            let mut chosen = None;
            for &id in &listener.bound {
                let entry = listener.connections.get(id).expect("a bound connection exists");
                if entry.phase != Phase::Closing && entry.phase != Phase::Closed {
                    chosen = Some(id);
                    break;
                }
            }
            if let Some(id) = chosen {
                finish(listener, env, sizes, id, None, effects);
            }
        }
        State::Idle | State::Starting | State::Listening | State::Closed => {}
    }
}

pub fn fire(listener: &mut Listener, env: &Env<Limits>, sizes: &Sizes, effects: &mut Queue<Effect>) {
    let Some(id) = listener.alarms.expire(env.now) else {
        return;
    };
    let entry = listener.connections.get(id).expect("alarms belong to bound connections");
    match entry.phase {
        Phase::Closing => abort(listener, env, sizes, id, effects),
        Phase::Securing | Phase::Opening | Phase::Pending => finish(listener, env, sizes, id, None, effects),
        Phase::Hello if entry.until <= env.now => finish(listener, env, sizes, id, None, effects),
        Phase::Open
            if entry.heard.saturating_add(env.limits.channel.silence) <= env.now
                || entry.stalled(env.now, &env.limits) =>
        {
            finish(listener, env, sizes, id, None, effects);
        }
        Phase::Hello | Phase::Open => {
            let entry = listener.connections.get_mut(id).expect("the binding remains");
            entry.pinged = env.now;
            entry.request(&env.limits, sizes, machine::Request::Send(wire::Message::Ping));
            discard(listener, env, id, effects);
        }
        Phase::Closed => {}
    }
}

fn padded(secret: &[u8]) -> Option<[u8; 65]> {
    if secret.len() > 64 {
        return None;
    }
    let mut bytes = [0; 65];
    *bytes.first_mut()? = u8::try_from(secret.len()).ok()?;
    for (index, &byte) in secret.iter().enumerate() {
        *bytes.get_mut(index.checked_add(1)?)? = byte;
    }
    Some(bytes)
}

fn authenticate(listener: &Listener, open: &wire::Open) -> Option<u32> {
    let offered = padded(&open.secret)?;
    let mut found = None;
    for (index, worker) in listener.workers.iter().enumerate() {
        let known = padded(&worker.secret).expect("configuration bounds secrets");
        let equal = bool::from(known.ct_eq(&offered));
        if worker.name == open.name && equal {
            found = Some(u32::try_from(index).expect("configured workers fit"));
        }
    }
    found
}

fn authorize(
    listener: &mut Listener,
    env: &Env<Limits>,
    sizes: &Sizes,
    id: Id<Connection>,
    open: wire::Open,
    effects: &mut Queue<Effect>,
) {
    let Some(worker) = authenticate(listener, &open) else {
        return finish(listener, env, sizes, id, Some(wire::Refuse { reason: 2, text: Box::new([]) }), effects);
    };
    let entry = listener.connections.get_mut(id).expect("authenticated connection remains bound");
    entry.worker = Some(worker);
    if let Some(&old) = listener.current.get(&worker) {
        // The newest authenticated Open wins the single pending slot. Earlier
        // pending links close as replaced, while the current link waits for io Closed.
        if let Some(prior) = listener.pending.insert(worker, id).expect("one pending link per worker") {
            finish(listener, env, sizes, prior, Some(wire::Refuse { reason: 5, text: Box::new([]) }), effects);
        }
        listener.connections.get_mut(id).expect("new pending binding remains").phase = Phase::Pending;
        finish(listener, env, sizes, old, Some(wire::Refuse { reason: 5, text: Box::new([]) }), effects);
    } else {
        activate(listener, env, sizes, worker, id, effects);
    }
}

fn activate(
    listener: &mut Listener,
    env: &Env<Limits>,
    sizes: &Sizes,
    worker: u32,
    id: Id<Connection>,
    effects: &mut Queue<Effect>,
) {
    let replaced = listener.current.insert(worker, id).expect("one current connection per configured worker");
    assert!(replaced.is_none(), "old current link is actually closed before accepting a replacement");
    let entry = listener.connections.get_mut(id).expect("authenticated binding remains");
    entry.phase = Phase::Opening;
    entry.request(&env.limits, sizes, machine::Request::Accept { version: 1 });
    lower(listener, env, id, effects);
    follow(listener, env, id);
}

fn drain(
    listener: &mut Listener,
    env: &Env<Limits>,
    sizes: &Sizes,
    id: Id<Connection>,
    out: &mut Queue<Notice>,
    effects: &mut Queue<Effect>,
) {
    lower(listener, env, id, effects);
    for _ in 0..machine::MAX_UP {
        let Some(event) = listener.connections.get_mut(id).expect("the binding remains").upper.pop() else {
            break;
        };
        match event {
            machine::Event::Message(wire::Message::Open { open }) => {
                authorize(listener, env, sizes, id, open, effects);
            }
            machine::Event::Message(message) => {
                let repositories =
                    u32::try_from(listener.repositories.len()).expect("configured deployment count fits");
                match translate::up(id.token(), message, repositories, sizes) {
                    Ok(Some(event)) => {
                        let entry = listener.connections.get_mut(id).expect("the binding remains");
                        match &event {
                            engine::Event::Hello { .. } => {
                                entry.phase = Phase::Open;
                                entry.announced = true;
                            }
                            engine::Event::Refreshed { .. }
                            | engine::Event::RefreshFailed { .. }
                            | engine::Event::Rejected { .. }
                            | engine::Event::Exhausted { .. }
                            | engine::Event::Answered { .. }
                            | engine::Event::Hint { .. }
                            | engine::Event::Lost { .. }
                            | engine::Event::Answer { .. }
                            | engine::Event::Relay { .. }
                            | engine::Event::Bounced { .. }
                            | engine::Event::Told { .. }
                            | engine::Event::Ask { .. }
                            | engine::Event::Unwatch { .. }
                            | engine::Event::Delivered { .. }
                            | engine::Event::Stored { .. } => {}
                        }
                        out.push(Notice::Domain(event));
                        entry.request(&env.limits, sizes, machine::Request::Read);
                    }
                    Ok(None) => listener.connections.get_mut(id).expect("the binding remains").request(
                        &env.limits,
                        sizes,
                        machine::Request::Read,
                    ),
                    Err(error @ translate::Error::InvalidRelay { .. }) => {
                        let reply = translate::reply(error, sizes).expect("the v1 Invalid answer fits");
                        let entry = listener.connections.get_mut(id).expect("the binding remains");
                        entry.request(&env.limits, sizes, machine::Request::Send(reply));
                        lower(listener, env, id, effects);
                        listener.connections.get_mut(id).expect("the binding remains").request(
                            &env.limits,
                            sizes,
                            machine::Request::Read,
                        );
                    }
                    Err(
                        translate::Error::Name
                        | translate::Error::Repository
                        | translate::Error::Credential
                        | translate::Error::Payload
                        | translate::Error::Direction,
                    ) => finish(listener, env, sizes, id, None, effects),
                }
            }
            machine::Event::Ready { version: _ } => {
                let entry = listener.connections.get_mut(id).expect("the binding remains");
                entry.phase = Phase::Hello;
                entry.until = env.now.saturating_add(env.limits.channel.hello);
            }
            machine::Event::Closed { fault: _ } => {
                let entry = listener.connections.get_mut(id).expect("the binding remains");
                if entry.phase != Phase::Closing {
                    entry.phase = Phase::Closing;
                    entry.until = env.now.saturating_add(env.limits.channel.stall);
                }
            }
            machine::Event::Sent | machine::Event::Unsent | machine::Event::ReadEnded => {}
        }
        lower(listener, env, id, effects);
    }
    assert!(
        listener.connections.get(id).expect("the binding remains").upper.is_empty(),
        "bounded machine output drained"
    );
    follow(listener, env, id);
}

/// A down-pass machine operation cannot produce a domain notification;
/// any late event is discarded once its connection begins closing.
fn discard(listener: &mut Listener, env: &Env<Limits>, id: Id<Connection>, effects: &mut Queue<Effect>) {
    lower(listener, env, id, effects);
    let entry = listener.connections.get_mut(id).expect("the binding remains");
    for _ in 0..machine::MAX_UP {
        let Some(event) = entry.upper.pop() else {
            break;
        };
        match event {
            machine::Event::Closed { fault: _ } => {
                entry.phase = Phase::Closing;
                entry.until = env.now.saturating_add(env.limits.channel.stall);
            }
            machine::Event::Message(_)
            | machine::Event::Ready { .. }
            | machine::Event::Sent
            | machine::Event::Unsent
            | machine::Event::ReadEnded => {}
        }
    }
    assert!(entry.upper.is_empty(), "bounded machine output discarded");
    follow(listener, env, id);
}

fn lower(listener: &mut Listener, env: &Env<Limits>, id: Id<Connection>, effects: &mut Queue<Effect>) {
    let entry = listener.connections.get_mut(id).expect("machine output keeps its binding");
    for _ in 0..machine::MAX_DOWN {
        let Some(down) = entry.lower.pop() else {
            break;
        };
        match &down {
            stream::Down::Send(_) => entry.sent = env.now,
            stream::Down::Demand { .. } | stream::Down::Finish => {}
        }
        let finish = down == stream::Down::Finish;
        match entry.transport {
            Transport::Loopback => effects.push(Effect::Io(io::Request::Stream { stream: entry.socket, down })),
            Transport::Secured => effects.push(Effect::Security(Security::Output { owner: id.token(), down })),
        }
        if finish {
            entry.phase = Phase::Closing;
            entry.until = env.now.saturating_add(env.limits.channel.stall);
            if entry.transport == Transport::Loopback {
                effects.push(Effect::Io(io::Request::Close { entity: entry.socket }));
            }
        }
    }
    assert!(entry.lower.is_empty(), "bounded lower output drained");
}

fn finish(
    listener: &mut Listener,
    env: &Env<Limits>,
    sizes: &Sizes,
    id: Id<Connection>,
    refusal: Option<wire::Refuse>,
    effects: &mut Queue<Effect>,
) {
    let Some(entry) = listener.connections.get_mut(id) else {
        return;
    };
    if entry.phase == Phase::Closing || entry.phase == Phase::Closed {
        return;
    }
    let securing = entry.phase == Phase::Securing;
    entry.phase = Phase::Closing;
    entry.until = env.now.saturating_add(env.limits.channel.stall);
    if securing {
        return abort(listener, env, sizes, id, effects);
    }
    let request = match refusal {
        Some(refuse) => machine::Request::Refuse(refuse),
        None => machine::Request::Close,
    };
    entry.request(&env.limits, sizes, request);
    discard(listener, env, id, effects);
}

fn abort(listener: &mut Listener, env: &Env<Limits>, sizes: &Sizes, id: Id<Connection>, effects: &mut Queue<Effect>) {
    let Some(entry) = listener.connections.get_mut(id) else {
        return;
    };
    if entry.phase == Phase::Closed || entry.aborted {
        return;
    }
    entry.phase = Phase::Closing;
    entry.aborted = true;
    entry.request(&env.limits, sizes, machine::Request::Close);
    discard(listener, env, id, effects);
    let entry = listener.connections.get(id).expect("abort retains the binding until Closed");
    if entry.transport == Transport::Secured {
        effects.push(Effect::Security(Security::Stop { owner: id.token() }));
    }
    effects.push(Effect::Io(io::Request::Abort { entity: entry.socket }));
    follow(listener, env, id);
}

fn follow(listener: &mut Listener, env: &Env<Limits>, id: Id<Connection>) {
    let entry = listener.connections.get_mut(id).expect("the binding remains");
    let debt = entry.machine.queued_bytes() > 0;
    if debt && !entry.debt {
        entry.progress = env.now;
    }
    entry.debt = debt;
    if entry.machine.is_ready() && entry.phase != Phase::Securing && entry.phase != Phase::Closed && !entry.aborted {
        listener.ready.insert(id).expect("one ready mark per connection");
    } else {
        listener.ready.remove(&id);
    }
    match entry.deadline(&env.limits) {
        Some(at) => listener.alarms.arm(id, at).expect("one alarm per bound connection"),
        None => listener.alarms.cancel(id),
    }
}
