//! Credential execution below the secret-free account domain.
//!
//! Io owner tokens are local to this component. A whole-engine loop must map
//! tagged component bindings to fresh global io owners, retaining each until
//! actual Closed. Socket handles remain opaque io-issued names.
//!
//! Keep is an explicit durability boundary: its owner must atomically replace
//! the versioned record, sync its file and directory, and only then answer
//! Kept. This kit has no file io; emitting Keep does not achieve durability.
use crate::{
    connection::Transport,
    credentials::{self, Config, Identity, Table},
    oauth_exchange as exchange,
};
use alloc::boxed::Box;
use skein_io as io;
use skein_lib::{Deadlines, Duration, Env, Id, Map, Queue, Set, Slab, Time, Token, Wall, bytes, stream};
use temper_legacy_engine_domain::{self as engine, accounts};

/// One HTTP entrypoint, a socket abort and a keeper request.
pub const MAX_OUT: u32 = 8;
pub const MAX_UP: u32 = 1;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Limits {
    pub credentials: credentials::Limits,
    pub operations: u32,
    pub http: skein_http::client::Limits,
    pub timeout: Duration,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    Limits,
    Credential(credentials::Error),
    Busy,
    Request,
}
#[expect(missing_debug_implementations, reason = "plaintext OAuth streams contain credential values")]
pub enum Security {
    Start { owner: Token, socket: Token, host: Box<[u8]> },
    Input { owner: Token, up: stream::Up },
    Output { owner: Token, down: stream::Down },
    Stop { owner: Token },
    Closed { owner: Token },
}
#[expect(missing_debug_implementations, reason = "keeper records and plaintext HTTP effects contain secrets")]
pub enum Effect {
    Io(io::Request),
    Security(Security),
    Keep { owner: Token, account: u32, generation: u64, record: Box<[u8]> },
    CancelKeep { owner: Token },
}
#[expect(missing_debug_implementations, reason = "plaintext OAuth stream events contain secrets")]
pub enum Event {
    Io(io::Event),
    Secured { owner: Token },
    Plain { owner: Token, up: stream::Up },
    SecurityFailed { owner: Token },
    Kept { owner: Token },
    Unkept { owner: Token },
    KeepCancelled { owner: Token },
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    Connecting,
    Securing,
    Http,
    Saving,
    Closing,
}
#[expect(
    clippy::struct_excessive_bools,
    reason = "socket and keeper settlement, cancellation and once-only effects are independent lifecycle fences"
)]
struct Operation {
    account: u32,
    generation: u64,
    phase: Phase,
    exchange: Option<exchange::Exchange>,
    socket: Option<Token>,
    transport: Transport,
    io_closed: bool,
    close_sent: bool,
    keeper: bool,
    cancel_sent: bool,
    cancelled: bool,
    until: Time,
}
#[expect(missing_debug_implementations, reason = "credential values, candidates and HTTP operations contain secrets")]
pub struct Owner {
    table: Table,
    operations: Slab<Operation>,
    active: Map<u32, Id<Operation>>,
    bound: Set<Id<Operation>>,
    ready: Set<Id<Operation>>,
    alarms: Deadlines<Id<Operation>>,
    upper: Queue<exchange::Event>,
    lower: Queue<stream::Down>,
}
impl Owner {
    pub fn new(
        configs: Box<[Config]>,
        identities: Box<[Identity]>,
        now: Time,
        wall: Wall,
        limits: &Limits,
        io_limits: &io::Limits,
    ) -> Result<Owner, Error> {
        if !fits_io(limits, io_limits) {
            return Err(Error::Limits);
        }
        let table = match Table::new(configs, identities, now, wall, &limits.credentials) {
            Ok(table) => table,
            Err(error) => return Err(Error::Credential(error)),
        };
        Ok(Owner {
            table,
            operations: Slab::with_capacity(limits.operations),
            active: Map::with_capacity(limits.credentials.accounts),
            bound: Set::with_capacity(limits.operations),
            ready: Set::with_capacity(limits.operations),
            alarms: Deadlines::with_capacity(limits.operations),
            upper: Queue::with_capacity(exchange::MAX_UP),
            lower: Queue::with_capacity(exchange::MAX_DOWN),
        })
    }
    #[must_use]
    pub fn table(&self) -> &Table {
        &self.table
    }
    #[must_use]
    pub fn is_ready(&self) -> bool {
        !self.ready.is_empty()
    }
    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        match (self.alarms.next(), self.table.next_deadline()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) | (None, Some(a)) => Some(a),
            (None, None) => None,
        }
    }
    #[must_use]
    pub fn is_due(&self, now: Time) -> bool {
        match self.next_deadline() {
            Some(at) => at <= now,
            None => false,
        }
    }
    #[must_use]
    pub fn bindings(&self) -> u32 {
        self.bound.len()
    }
    #[must_use]
    pub fn operation(&self, account: u32) -> Option<Token> {
        Some(self.active.get(&account)?.token())
    }
    pub fn reclaim(&mut self) {
        self.operations.reclaim();
    }
}
pub fn down(
    owner: &mut Owner,
    env: &Env<Limits>,
    request: accounts::Request,
    above: &mut Queue<engine::Event>,
    effects: &mut Queue<Effect>,
) -> Result<(), Error> {
    match request {
        accounts::Request::Refresh { account, generation } => {
            begin(owner, env, account, generation, false, above, effects)
        }
        accounts::Request::Keep { account, generation } => begin(owner, env, account, generation, true, above, effects),
        accounts::Request::Cancel { account, generation } => {
            cancel(owner, env, account, generation, effects);
            Ok(())
        }
        accounts::Request::Granted { .. }
        | accounts::Request::Availability { .. }
        | accounts::Request::Refused { .. }
        | accounts::Request::Closed { .. } => Err(Error::Request),
    }
}
fn begin(
    owner: &mut Owner,
    env: &Env<Limits>,
    account: u32,
    generation: u64,
    keep: bool,
    above: &mut Queue<engine::Event>,
    effects: &mut Queue<Effect>,
) -> Result<(), Error> {
    if owner.active.get(&account).is_some() {
        return Err(Error::Busy);
    }
    let endpoint = owner.table.endpoint(account).ok_or(Error::Credential(credentials::Error::Account))?;
    let transport = endpoint.transport;
    let address = endpoint.address;
    if owner.operations.is_full() {
        if !keep {
            match owner.table.validate_refresh(account, generation) {
                Ok(()) => {}
                Err(error) => return Err(Error::Credential(error)),
            }
        }
        let failure = if keep {
            accounts::Failure::Unsaved {
                valid: match owner.table.candidate_valid(account, generation, env.now) {
                    Ok(valid) => valid,
                    Err(error) => return Err(Error::Credential(error)),
                },
            }
        } else {
            accounts::Failure::Unavailable
        };
        above.push(engine::Event::RefreshFailed { account, generation, failure });
        return Ok(());
    }
    let (attempt, record) = if keep {
        let record = match owner.table.record(account, generation, &env.limits.credentials) {
            Ok(record) => record,
            Err(error) => return Err(Error::Credential(error)),
        };
        (None, Some(record))
    } else {
        let request = match owner.table.request(account, generation) {
            Ok(request) => request,
            Err(error) => return Err(Error::Credential(error)),
        };
        let attempt =
            match exchange::Exchange::new(&request, &endpoint.host, &endpoint.path, &exchange_limits(&env.limits)) {
                Ok(attempt) => attempt,
                Err(error) => return Err(Error::Credential(credentials::Error::Document(error))),
            };
        (Some(attempt), None)
    };
    let operation = Operation {
        account,
        generation,
        phase: if keep { Phase::Saving } else { Phase::Connecting },
        exchange: attempt,
        socket: None,
        transport,
        io_closed: keep,
        close_sent: false,
        keeper: keep,
        cancel_sent: false,
        cancelled: false,
        until: env.now.saturating_add(env.limits.timeout),
    };
    let Ok(id) = owner.operations.insert(operation) else {
        unreachable!("operation entrance reserved a slot");
    };
    owner.active.insert(account, id).expect("at most one active operation per configured account");
    let inserted = owner.bound.insert(id).expect("one binding per operation");
    assert!(inserted, "fresh operation binding");
    owner.alarms.arm(id, env.now.saturating_add(env.limits.timeout)).expect("one timer per operation");
    if let Some(record) = record {
        effects.push(Effect::Keep { owner: id.token(), account, generation, record });
    } else {
        effects.push(Effect::Io(io::Request::Connect { owner: id.token(), addr: address }));
    }
    Ok(())
}
pub fn up(
    owner: &mut Owner,
    env: &Env<Limits>,
    event: Event,
    above: &mut Queue<engine::Event>,
    effects: &mut Queue<Effect>,
) {
    match event {
        Event::Io(event) => io_up(owner, env, event, above, effects),
        Event::Secured { owner: token } => {
            let id = Id::from_token(token);
            let Some(operation) = owner.operations.get_mut(id) else {
                return;
            };
            if operation.phase != Phase::Securing {
                return;
            }
            operation.phase = Phase::Http;
            mark(owner, id);
        }
        Event::Plain { owner: token, up } => input(owner, env, Id::from_token(token), up, true, effects),
        Event::SecurityFailed { owner: token } => {
            let id = Id::from_token(token);
            let Some(operation) = owner.operations.get(id) else {
                return;
            };
            let failure = transport_failure(operation);
            fail(owner, env, id, failure, above, effects);
        }
        Event::Kept { owner: token } => keeper(owner, env, Id::from_token(token), true, above, effects),
        Event::Unkept { owner: token } | Event::KeepCancelled { owner: token } => {
            keeper(owner, env, Id::from_token(token), false, above, effects);
        }
    }
}
fn io_up(
    owner: &mut Owner,
    env: &Env<Limits>,
    event: io::Event,
    above: &mut Queue<engine::Event>,
    effects: &mut Queue<Effect>,
) {
    match event {
        io::Event::Connecting { owner: token, socket } => {
            let id = Id::from_token(token);
            let Some(operation) = owner.operations.get_mut(id) else {
                return;
            };
            if operation.io_closed || operation.socket.is_some() {
                return;
            }
            operation.socket = Some(socket);
            if operation.phase == Phase::Closing || operation.phase == Phase::Saving {
                abort(owner, id, effects);
            }
        }
        io::Event::Connected { owner: token } => {
            let id = Id::from_token(token);
            let Some(operation) = owner.operations.get_mut(id) else {
                return;
            };
            if operation.phase != Phase::Connecting {
                return;
            }
            match operation.transport {
                Transport::Loopback => {
                    operation.phase = Phase::Http;
                    mark(owner, id);
                }
                Transport::Secured => {
                    operation.phase = Phase::Securing;
                    let host =
                        bytes::copy_of(&owner.table.endpoint(operation.account).expect("configured account").host);
                    effects.push(Effect::Security(Security::Start {
                        owner: token,
                        socket: operation.socket.expect("Connecting precedes Connected"),
                        host,
                    }));
                }
            }
        }
        io::Event::Stream { owner: token, up } => {
            let id = Id::from_token(token);
            let Some(operation) = owner.operations.get(id) else {
                return;
            };
            if operation.io_closed {
                return;
            }
            match operation.transport {
                Transport::Loopback => input(owner, env, id, up, false, effects),
                Transport::Secured => {
                    if operation.phase == Phase::Securing || operation.phase == Phase::Http {
                        effects.push(Effect::Security(Security::Input { owner: token, up }));
                    }
                }
            }
        }
        io::Event::Failed { owner: token, error: _ } => {
            let id = Id::from_token(token);
            let Some(operation) = owner.operations.get(id) else {
                return;
            };
            let failure = transport_failure(operation);
            fail(owner, env, id, failure, above, effects);
        }
        io::Event::Closed { owner: token } => {
            let id = Id::from_token(token);
            let Some(operation) = owner.operations.get_mut(id) else {
                return;
            };
            if operation.io_closed {
                return;
            }
            operation.io_closed = true;
            operation.socket = None;
            let phase = operation.phase;
            if operation.transport == Transport::Secured {
                effects.push(Effect::Security(Security::Closed { owner: token }));
            }
            if phase == Phase::Closing && operation.cancelled && owner.active.get(&operation.account) == Some(&id) {
                above.push(engine::Event::RefreshFailed {
                    account: operation.account,
                    generation: operation.generation,
                    failure: accounts::Failure::Cancelled,
                });
                finish(owner, env, id, effects);
                return;
            }
            match phase {
                Phase::Connecting | Phase::Securing => {
                    fail(owner, env, id, accounts::Failure::Unavailable, above, effects);
                }
                Phase::Http => {
                    let failure = transport_failure(owner.operations.get(id).expect("closed binding retained"));
                    fail(owner, env, id, failure, above, effects);
                }
                Phase::Saving | Phase::Closing => retire(owner, id),
            }
        }
        io::Event::Listening { .. } | io::Event::Accepted { .. } => {}
    }
}
fn input(
    owner: &mut Owner,
    env: &Env<Limits>,
    id: Id<Operation>,
    up: stream::Up,
    secured: bool,
    effects: &mut Queue<Effect>,
) {
    let Some(operation) = owner.operations.get_mut(id) else {
        return;
    };
    if operation.phase != Phase::Http || (operation.transport == Transport::Secured) != secured {
        return;
    }
    let child = child_env(env);
    exchange::up(operation.exchange.as_mut().expect("HTTP attempt exists"), &child, up, &mut owner.lower);
    lower(owner, id, effects);
    follow(owner, id);
}
pub fn resume(owner: &mut Owner, env: &Env<Limits>, above: &mut Queue<engine::Event>, effects: &mut Queue<Effect>) {
    let Some(id) = owner.ready.first().copied() else {
        return;
    };
    owner.ready.remove(&id);
    let Some(operation) = owner.operations.get_mut(id) else {
        return;
    };
    if operation.phase != Phase::Http {
        return;
    }
    exchange::resume(
        operation.exchange.as_mut().expect("HTTP attempt exists"),
        &child_env(env),
        &mut owner.upper,
        &mut owner.lower,
    );
    lower(owner, id, effects);
    if let Some(event) = owner.upper.pop() {
        match event {
            exchange::Event::Completed(response) => {
                let operation = owner.operations.get(id).expect("active HTTP binding");
                let account = operation.account;
                let generation = operation.generation;
                match owner.table.prepare(account, generation, response, env.now, env.wall, &env.limits.credentials) {
                    Ok(_) => {
                        let record = owner
                            .table
                            .record(account, generation, &env.limits.credentials)
                            .expect("validated candidate encodes a saved record");
                        let operation = owner.operations.get_mut(id).expect("active HTTP binding");
                        operation.phase = Phase::Saving;
                        operation.keeper = true;
                        operation.until = env.now.saturating_add(env.limits.timeout);
                        owner.alarms.arm(id, operation.until).expect("one timer per operation");
                        effects.push(Effect::Keep { owner: id.token(), account, generation, record });
                        abort(owner, id, effects);
                    }
                    Err(_) => fail(owner, env, id, accounts::Failure::TimedOut, above, effects),
                }
            }
            exchange::Event::Failed(failure) => fail(owner, env, id, account_failure(failure), above, effects),
        }
    }
    follow(owner, id);
}
fn keeper(
    owner: &mut Owner,
    env: &Env<Limits>,
    id: Id<Operation>,
    kept: bool,
    above: &mut Queue<engine::Event>,
    effects: &mut Queue<Effect>,
) {
    let Some(operation) = owner.operations.get_mut(id) else {
        return;
    };
    if operation.phase != Phase::Saving || !operation.keeper {
        return;
    }
    operation.keeper = false;
    let account = operation.account;
    let generation = operation.generation;
    let cancellation = operation.cancelled;
    let result = if kept {
        Some(owner.table.saved(account, generation, env.now).expect("keeper answers the held candidate"))
    } else {
        None
    };
    if cancellation {
        above.push(engine::Event::RefreshFailed { account, generation, failure: accounts::Failure::Cancelled });
    } else if let Some(valid) = result {
        above.push(engine::Event::Refreshed { account, generation, valid });
    } else {
        above.push(engine::Event::RefreshFailed {
            account,
            generation,
            failure: accounts::Failure::Unsaved {
                valid: owner.table.candidate_valid(account, generation, env.now).expect("unsaved candidate retained"),
            },
        });
    }
    finish(owner, env, id, effects);
}
fn cancel(owner: &mut Owner, env: &Env<Limits>, account: u32, generation: u64, effects: &mut Queue<Effect>) {
    let Some(id) = owner.active.get(&account).copied() else {
        return;
    };
    let operation = owner.operations.get_mut(id).expect("active operation exists");
    if operation.generation != generation {
        return;
    }
    operation.cancelled = true;
    match operation.phase {
        Phase::Saving => {
            if !operation.cancel_sent {
                operation.cancel_sent = true;
                effects.push(Effect::CancelKeep { owner: id.token() });
            }
        }
        Phase::Connecting | Phase::Securing | Phase::Http => {
            operation.phase = Phase::Closing;
            owner.ready.remove(&id);
            owner.alarms.cancel(id);
            if let Some(attempt) = &mut operation.exchange {
                exchange::close(attempt, &child_env(env), &mut owner.lower);
            }
            lower(owner, id, effects);
            abort(owner, id, effects);
        }
        Phase::Closing => {}
    }
}
fn fail(
    owner: &mut Owner,
    env: &Env<Limits>,
    id: Id<Operation>,
    failure: accounts::Failure,
    above: &mut Queue<engine::Event>,
    effects: &mut Queue<Effect>,
) {
    let Some(operation) = owner.operations.get(id) else {
        return;
    };
    match operation.phase {
        Phase::Saving | Phase::Closing => return,
        Phase::Connecting | Phase::Securing | Phase::Http => {}
    }
    above.push(engine::Event::RefreshFailed { account: operation.account, generation: operation.generation, failure });
    finish(owner, env, id, effects);
}
fn finish(owner: &mut Owner, env: &Env<Limits>, id: Id<Operation>, effects: &mut Queue<Effect>) {
    let operation = owner.operations.get_mut(id).expect("terminal operation binding");
    operation.phase = Phase::Closing;
    let removed = owner.active.remove(&operation.account);
    assert!(removed == Some(id), "terminal removes its active account operation");
    owner.ready.remove(&id);
    owner.alarms.cancel(id);
    if let Some(attempt) = &mut operation.exchange {
        exchange::close(attempt, &child_env(env), &mut owner.lower);
    }
    lower(owner, id, effects);
    abort(owner, id, effects);
    retire(owner, id);
}
fn abort(owner: &mut Owner, id: Id<Operation>, effects: &mut Queue<Effect>) {
    let Some(operation) = owner.operations.get_mut(id) else {
        return;
    };
    if operation.close_sent || operation.io_closed {
        return;
    }
    let Some(socket) = operation.socket else {
        return;
    };
    operation.close_sent = true;
    if operation.transport == Transport::Secured {
        effects.push(Effect::Security(Security::Stop { owner: id.token() }));
    }
    effects.push(Effect::Io(io::Request::Abort { entity: socket }));
}
fn lower(owner: &mut Owner, id: Id<Operation>, effects: &mut Queue<Effect>) {
    let operation = owner.operations.get(id).expect("HTTP output binding exists");
    for _effect in 0..owner.lower.capacity() {
        let Some(down) = owner.lower.pop() else {
            break;
        };
        if operation.io_closed {
            continue;
        }
        match operation.transport {
            Transport::Loopback => effects.push(Effect::Io(io::Request::Stream {
                stream: operation.socket.expect("HTTP stream connected"),
                down,
            })),
            Transport::Secured => effects.push(Effect::Security(Security::Output { owner: id.token(), down })),
        }
    }
}
fn mark(owner: &mut Owner, id: Id<Operation>) {
    owner.ready.insert(id).expect("one ready mark per operation");
}
fn follow(owner: &mut Owner, id: Id<Operation>) {
    let Some(operation) = owner.operations.get(id) else {
        return;
    };
    if operation.phase == Phase::Http && operation.exchange.as_ref().expect("HTTP attempt exists").is_ready() {
        mark(owner, id);
    }
}
fn retire(owner: &mut Owner, id: Id<Operation>) {
    let Some(operation) = owner.operations.get(id) else {
        return;
    };
    if operation.phase != Phase::Closing || !operation.io_closed || operation.keeper {
        return;
    }
    let removed = owner.bound.remove(&id);
    assert!(removed, "actual terminals release one binding");
    owner.operations.retire(id);
}
pub fn fire(owner: &mut Owner, env: &Env<Limits>, above: &mut Queue<engine::Event>, effects: &mut Queue<Effect>) {
    owner.table.expire(env.now);
    let Some(id) = owner.alarms.expire(env.now) else {
        return;
    };
    let Some(operation) = owner.operations.get_mut(id) else {
        return;
    };
    match operation.phase {
        Phase::Saving => {
            if !operation.cancel_sent {
                operation.cancel_sent = true;
                effects.push(Effect::CancelKeep { owner: id.token() });
            }
        }
        Phase::Connecting | Phase::Securing | Phase::Http => {
            let failure = match operation.exchange.as_ref() {
                Some(attempt) if attempt.started() => accounts::Failure::TimedOut,
                Some(_) | None => accounts::Failure::Unavailable,
            };
            fail(owner, env, id, failure, above, effects);
        }
        Phase::Closing => {}
    }
}
fn transport_failure(operation: &Operation) -> accounts::Failure {
    match operation.exchange.as_ref() {
        Some(attempt) if attempt.started() => accounts::Failure::TimedOut,
        Some(_) | None => accounts::Failure::Unavailable,
    }
}
fn account_failure(failure: temper_oauth::Failure) -> accounts::Failure {
    match failure {
        temper_oauth::Failure::Unavailable => accounts::Failure::Unavailable,
        temper_oauth::Failure::TimedOut => accounts::Failure::TimedOut,
        temper_oauth::Failure::RateLimited { retry_after } => accounts::Failure::RateLimited { retry_after },
        temper_oauth::Failure::Refused => accounts::Failure::Refused,
    }
}
fn child_env(env: &Env<Limits>) -> Env<exchange::Limits> {
    Env { now: env.now, wall: env.wall, limits: exchange_limits(&env.limits) }
}
const fn exchange_limits(limits: &Limits) -> exchange::Limits {
    exchange::Limits { http: limits.http, documents: limits.credentials.documents }
}
#[must_use]
pub fn fits_io(limits: &Limits, io_limits: &io::Limits) -> bool {
    let Some(records) = output_records(limits) else {
        return false;
    };
    worst_case(limits).is_some()
        && io_limits.is_usable()
        && io::worst_case(io_limits).is_some()
        && skein_http::client::largest_read(&limits.http) <= io_limits.largest_read()
        && skein_http::client::largest_room(&limits.http) <= io_limits.largest_room()
        && io_limits.sends.saturating_add(1) >= records
}
/// One whole request head and every bounded upload chunk may still be below
/// the HTTP machine. A secured stream's owner must separately admit its TLS
/// encrypted records; this is the concrete plaintext record bound.
#[must_use]
pub fn output_records(limits: &Limits) -> Option<u32> {
    let whole = limits.credentials.documents.document_bytes.checked_div(limits.http.send)?;
    whole
        .checked_add(u32::from(!limits.credentials.documents.document_bytes.is_multiple_of(limits.http.send)))?
        .checked_add(1)
}
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    if limits.operations < limits.credentials.accounts || limits.operations == 0 || limits.timeout == Duration::ZERO {
        return None;
    }
    credentials::worst_case(&limits.credentials)?
        .checked_add(Slab::<Operation>::worst_case(limits.operations)?)?
        .checked_add(Map::<u32, Id<Operation>>::worst_case(limits.credentials.accounts)?)?
        .checked_add(Set::<Id<Operation>>::worst_case(limits.operations)?.checked_mul(2)?)?
        .checked_add(Deadlines::<Id<Operation>>::worst_case(limits.operations)?)?
        .checked_add(exchange::worst_case(&exchange_limits(limits))?.checked_mul(u64::from(limits.operations))?)?
        .checked_add(Queue::<exchange::Event>::worst_case(exchange::MAX_UP)?)?
        .checked_add(Queue::<stream::Down>::worst_case(exchange::MAX_DOWN)?)?
        .checked_add(
            u64::from(limits.credentials.documents.record_bytes)
                .checked_mul(u64::from(limits.operations).checked_add(2)?)?,
        )
}
