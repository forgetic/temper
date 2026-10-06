//! One dial attempt, owned until io's actual Closed, with waits across links.
use crate::{Limits, credentials, relays, translate};
use alloc::boxed::Box;
use skein_lib::{Env, Queue, Time, Token, bytes::copy_of, stream};
use temper_channel::{
    codec,
    machine::{self, Endpoint, Machine},
    wire,
};
use temper_worker_domain::{self as worker};

pub const MAX_UP: u32 = 2;
/// One child batch, then a possible fault-close batch, then the io Close.
pub const MAX_DOWN: u32 = machine::MAX_DOWN * 2 + 1;
#[expect(missing_debug_implementations, reason = "authentication secrets must never occur in traces")]
pub struct Config {
    pub address: skein_io::kernel::Addr,
    pub name: Box<[u8]>,
    pub secret: Box<[u8]>,
    pub accounts: Box<[u32]>,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Idle,
    Connecting,
    Opening,
    Open,
    Closing,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    Config,
    State,
    Credential(credentials::Error),
    Translation(translate::Error),
    Relay,
}
#[expect(missing_debug_implementations, reason = "authentication and queued grant values must never occur in traces")]
pub struct Link {
    owner: Token,
    config: Config,
    state: State,
    socket: Option<Token>,
    machine: Option<Machine>,
    handshake: Option<Time>,
    last_sent: Time,
    last_ping: Time,
    close_sent: bool,
    fact_records: u32,
    fact_bytes: u32,
    read_again: bool,
    last_heard: Time,
    stalled: Option<Time>,
    events: Queue<machine::Event>,
    lower: Queue<stream::Down>,
    credentials: credentials::Table,
    relays: relays::Table,
}
impl Link {
    pub fn new(owner: Token, config: Config, limits: &Limits, io: &skein_io::Limits) -> Result<Link, Error> {
        if !limits.fits_io(io)
            || !config.address.ip().is_loopback()
            || config.name.is_empty()
            || config.secret.is_empty()
            || u32::try_from(config.name.len()).ok().ok_or(Error::Config)? > limits.channel.name_bytes
            || u32::try_from(config.secret.len()).ok().ok_or(Error::Config)? > limits.channel.secret_bytes
        {
            return Err(Error::Config);
        }
        let credentials = match credentials::Table::new(&config.accounts, limits.accounts, limits.sizes.token_bytes) {
            Ok(table) => table,
            Err(error) => return Err(Error::Credential(error)),
        };
        Ok(Link {
            owner,
            config,
            state: State::Idle,
            socket: None,
            machine: None,
            handshake: None,
            last_sent: Time::ZERO,
            last_ping: Time::ZERO,
            close_sent: false,
            fact_records: 0,
            fact_bytes: 0,
            read_again: false,
            last_heard: Time::ZERO,
            stalled: None,
            events: Queue::with_capacity(machine::MAX_UP),
            lower: Queue::with_capacity(machine::MAX_DOWN),
            credentials,
            relays: relays::Table::new(limits.relays),
        })
    }
    #[must_use]
    pub const fn state(&self) -> State {
        self.state
    }
    #[must_use]
    pub const fn credentials(&self) -> &credentials::Table {
        &self.credentials
    }
    #[must_use]
    pub const fn relays(&self) -> &relays::Table {
        &self.relays
    }
    #[must_use]
    pub fn is_ready(&self) -> bool {
        !self.lower.is_empty()
            || !self.events.is_empty()
            || (self.read_again && self.state == State::Open)
            || match &self.machine {
                Some(machine) => machine.is_ready(),
                None => false,
            }
    }
    #[must_use]
    pub fn next_deadline(&self, limits: &Limits) -> Option<Time> {
        let mut at = self.credentials.next_deadline();
        if let Some(time) = self.handshake {
            at = Some(earlier(at, time));
        }
        if let Some(time) = self.stalled {
            at = Some(earlier(at, time));
        }
        if self.state == State::Open {
            at = Some(earlier(at, self.last_sent.max(self.last_ping).saturating_add(limits.channel.ping)));
            at = Some(earlier(at, self.last_heard.saturating_add(limits.channel.silence)));
        }
        at
    }
}
fn earlier(at: Option<Time>, candidate: Time) -> Time {
    match at {
        Some(at) => at.min(candidate),
        None => candidate,
    }
}
fn close(link: &mut Link, env: &Env<Limits>) {
    if link.state == State::Idle || link.state == State::Closing {
        return;
    }
    link.state = State::Closing;
    link.handshake = None;
    link.stalled = None;
    if let Some(machine) = &mut link.machine {
        machine::down(
            machine,
            &env.limits.channel,
            &env.limits.sizes,
            machine::Request::Close,
            &mut link.events,
            &mut link.lower,
        );
    }
}
fn flush(link: &mut Link, env: &Env<Limits>, lower: &mut Queue<skein_io::Request>) {
    for _ in 0..machine::MAX_DOWN {
        let Some(down) = link.lower.pop() else {
            break;
        };
        match &down {
            stream::Down::Send(_) => link.last_sent = env.now,
            stream::Down::Demand { .. } | stream::Down::Finish => {}
        }
        if let Some(socket) = link.socket {
            lower.push(skein_io::Request::Stream { stream: socket, down });
        }
    }
    if link.state == State::Closing
        && !link.close_sent
        && let Some(socket) = link.socket
    {
        link.close_sent = true;
        lower.push(skein_io::Request::Close { entity: socket });
    }
}
fn events(link: &mut Link, env: &Env<Limits>, out: &mut Queue<worker::Event>) {
    for _ in 0..machine::MAX_UP {
        let Some(event) = link.events.pop() else {
            break;
        };
        if link.state == State::Idle || link.state == State::Closing {
            drop(event);
            continue;
        }
        match event {
            machine::Event::Ready { .. } => {
                link.state = State::Open;
                link.handshake = None;
                link.last_heard = env.now;
                out.push(worker::Event::Connected);
            }
            machine::Event::Message(message) => {
                let incoming = translate::link::up(message, &env.limits.sizes);
                match incoming {
                    Ok(incoming) => {
                        for grant in incoming.grants {
                            if link.credentials.insert(grant, env.now, env.limits.skew).is_err() {
                                close(link, env);
                                return;
                            }
                        }
                        if let Some(event) = incoming.event {
                            let deliver = match &event {
                                worker::Event::ConnectedV2
                                | worker::Event::RelayedV2 { .. }
                                | worker::Event::AssignV2 { .. }
                                | worker::Event::AcknowledgeTurn { .. }
                                | worker::Event::TurnBusy { .. } => {
                                    unreachable!("version one decoding cannot produce a version two event")
                                }
                                worker::Event::Relayed { run, attempt, call, .. } => {
                                    link.relays.answer(*call, *run, *attempt)
                                }
                                worker::Event::Connected
                                | worker::Event::Lost
                                | worker::Event::Assign { .. }
                                | worker::Event::Inbound { .. }
                                | worker::Event::Cancel { .. }
                                | worker::Event::Grant { .. }
                                | worker::Event::RelayCancelled { .. }
                                | worker::Event::Acknowledged { .. }
                                | worker::Event::Shutdown
                                | worker::Event::Spawned { .. }
                                | worker::Event::Unspawned { .. }
                                | worker::Event::Sent { .. }
                                | worker::Event::Unsent { .. }
                                | worker::Event::Received { .. }
                                | worker::Event::Malformed { .. }
                                | worker::Event::Hangup { .. }
                                | worker::Event::Signalled { .. }
                                | worker::Event::Exited { .. }
                                | worker::Event::Reaped { .. }
                                | worker::Event::Done { .. } => true,
                            };
                            if deliver {
                                out.push(event);
                            }
                        }
                        link.read_again = true;
                    }
                    Err(
                        translate::Error::Direction
                        | translate::Error::Limits
                        | translate::Error::Credential
                        | translate::Error::Unsupported,
                    ) => {
                        close(link, env);
                        return;
                    }
                }
            }
            machine::Event::Closed { .. } => {
                close(link, env);
                return;
            }
            machine::Event::Sent | machine::Event::Unsent | machine::Event::ReadEnded => {}
        }
    }
    // A demand emitted while translating a frame goes down on the next
    // bounded resume; never recursively drain an unbounded frame tape.
    if let Some(machine) = &link.machine {
        if machine.queued_bytes() == 0 {
            link.stalled = None;
            link.fact_records = 0;
            link.fact_bytes = 0;
        } else if link.stalled.is_none() {
            link.stalled = Some(env.now.saturating_add(env.limits.channel.stall));
        }
    }
}
fn drain(link: &mut Link, env: &Env<Limits>, out: &mut Queue<worker::Event>, lower: &mut Queue<skein_io::Request>) {
    // Child output is emptied before a translated failure invokes Close;
    // each finite child batch has its own full internal scratch capacity.
    flush(link, env, lower);
    events(link, env, out);
    flush(link, env, lower);
}
/// Route only link requests here; process/file requests stay with their future io adapters.
pub fn down(
    link: &mut Link,
    env: &Env<Limits>,
    request: worker::Request,
    out: &mut Queue<worker::Event>,
    lower: &mut Queue<skein_io::Request>,
) -> Result<(), Error> {
    match request {
        worker::Request::HelloV2 { .. }
        | worker::Request::Turn { .. }
        | worker::Request::AnswerV2 { .. }
        | worker::Request::RelayV2 { .. } => return Err(Error::Translation(translate::Error::Unsupported)),
        worker::Request::Dial => {
            if link.state != State::Idle {
                return Err(Error::State);
            }
            link.state = State::Connecting;
            link.close_sent = false;
            link.handshake = Some(env.now.saturating_add(env.limits.channel.handshake));
            lower.push(skein_io::Request::Connect { owner: link.owner, addr: link.config.address });
            return Ok(());
        }
        worker::Request::CancelRelay { call } => {
            if link.relays.cancel(call) {
                out.push(worker::Event::RelayCancelled { call });
            }
            return Ok(());
        }
        request @ (worker::Request::Hello { .. }
        | worker::Request::Answer { .. }
        | worker::Request::Relay { .. }
        | worker::Request::Bounced { .. }
        | worker::Request::Rejected { .. }
        | worker::Request::Exhausted { .. }) => {
            let message = match translate::link::down(request, &env.limits.sizes) {
                Ok(Some(message)) => message,
                Ok(None) => return Err(Error::State),
                Err(error) => {
                    close(link, env);
                    drain(link, env, out, lower);
                    return Err(Error::Translation(error));
                }
            };
            match &message {
                wire::Message::Relay { run, attempt, call, .. } => {
                    if !link.relays.open(Token::new(*call), Token::new(*run), Token::new(*attempt)) {
                        return Err(Error::Relay);
                    }
                }
                wire::Message::Unsupported { .. }
                | wire::Message::HelloV2 { .. }
                | wire::Message::AnswerV2 { .. }
                | wire::Message::AssignV2 { .. }
                | wire::Message::AgentCallV2 { .. }
                | wire::Message::FinishV2 { .. }
                | wire::Message::AgentStartV2 { .. }
                | wire::Message::Turn { .. }
                | wire::Message::AcknowledgeTurn { .. }
                | wire::Message::TurnBusy { .. }
                | wire::Message::AgentTurn { .. }
                | wire::Message::Open { .. }
                | wire::Message::Accept { .. }
                | wire::Message::Refuse { .. }
                | wire::Message::Ping
                | wire::Message::Terms { .. }
                | wire::Message::Hello { .. }
                | wire::Message::Answer { .. }
                | wire::Message::Bounced { .. }
                | wire::Message::Told { .. }
                | wire::Message::Rejected { .. }
                | wire::Message::Exhausted { .. }
                | wire::Message::Assign { .. }
                | wire::Message::Inbound { .. }
                | wire::Message::Cancel { .. }
                | wire::Message::Relayed { .. }
                | wire::Message::Acknowledge { .. }
                | wire::Message::Grant { .. }
                | wire::Message::AgentCall { .. }
                | wire::Message::Withdraw { .. }
                | wire::Message::Fact { .. }
                | wire::Message::Long { .. }
                | wire::Message::LongDone
                | wire::Message::Waiting { .. }
                | wire::Message::Finish { .. }
                | wire::Message::AgentRejected { .. }
                | wire::Message::AgentExhausted { .. }
                | wire::Message::AgentStart { .. }
                | wire::Message::AgentEvent { .. }
                | wire::Message::AgentAnswer { .. }
                | wire::Message::AgentCancel
                | wire::Message::AgentGrant { .. } => {}
            }
            send(link, env, message, out, lower);
        }
        worker::Request::Spawn { .. }
        | worker::Request::Send { .. }
        | worker::Request::Read { .. }
        | worker::Request::Signal { .. }
        | worker::Request::Wait { .. }
        | worker::Request::Reap { .. }
        | worker::Request::Io { .. }
        | worker::Request::CancelIo { .. } => return Err(Error::Translation(translate::Error::Direction)),
    }
    Ok(())
}
fn send(
    link: &mut Link,
    env: &Env<Limits>,
    message: wire::Message,
    out: &mut Queue<worker::Event>,
    lower: &mut Queue<skein_io::Request>,
) {
    if link.state != State::Open {
        return;
    }
    if let Some(machine) = &mut link.machine {
        machine::down(
            machine,
            &env.limits.channel,
            &env.limits.sizes,
            machine::Request::Send(message),
            &mut link.events,
            &mut link.lower,
        );
    }
    drain(link, env, out, lower);
}
pub fn told(
    link: &mut Link,
    env: &Env<Limits>,
    value: worker::Told,
    out: &mut Queue<worker::Event>,
    lower: &mut Queue<skein_io::Request>,
) -> Result<bool, Error> {
    let message = match translate::link::told(value, &env.limits.sizes) {
        Ok(message) => message,
        Err(error) => return Err(Error::Translation(error)),
    };
    let length = codec::frame_len(&message, &env.limits.sizes).expect("translation validated");
    if !fact_fits(link, &env.limits, length) {
        return Ok(false);
    }
    link.fact_records = link.fact_records.checked_add(1).expect("fact admission counted");
    link.fact_bytes = link.fact_bytes.checked_add(length).expect("fact bytes admitted");
    send(link, env, message, out, lower);
    Ok(true)
}
/// Check before pulling a best-effort fact from the domain. Reserving its
/// largest frame avoids consuming a fact which cannot yet be queued.
#[must_use]
pub fn can_tell(link: &Link, limits: &Limits) -> bool {
    fact_fits(link, limits, temper_channel::sizes::frame(0x105, &limits.sizes).expect("startup validated"))
}
fn fact_fits(link: &Link, limits: &Limits, length: u32) -> bool {
    let frame = temper_channel::sizes::frame(0x105, &limits.sizes).expect("startup validated");
    let budget = limits.sizes.facts.checked_mul(frame).expect("startup validated");
    let reserve = temper_channel::sizes::output_cap(Endpoint::WorkerLink, &limits.sizes)
        .expect("startup validated")
        .checked_sub(budget)
        .expect("fact contribution");
    let room = match &link.machine {
        Some(machine) => machine.room(),
        None => 0,
    };
    link.state == State::Open
        && link.fact_records < limits.sizes.facts
        && length <= budget.saturating_sub(link.fact_bytes)
        && room.saturating_sub(reserve) >= length
}
/// All io terminal records retain their binding until Closed, including failed dials.
pub fn up(
    link: &mut Link,
    env: &Env<Limits>,
    event: skein_io::Event,
    out: &mut Queue<worker::Event>,
    lower: &mut Queue<skein_io::Request>,
) {
    match event {
        skein_io::Event::Connecting { owner, socket } => {
            if owner == link.owner && link.state != State::Idle {
                link.socket = Some(socket);
                if link.state == State::Closing {
                    link.close_sent = true;
                    lower.push(skein_io::Request::Abort { entity: socket });
                }
            }
        }
        skein_io::Event::Connected { owner } => {
            if owner != link.owner || link.state != State::Connecting {
                return;
            }
            let mut machine =
                Machine::new(Endpoint::WorkerLink, &env.limits.channel, &env.limits.sizes).expect("startup validated");
            machine::down(
                &mut machine,
                &env.limits.channel,
                &env.limits.sizes,
                machine::Request::Send(wire::Message::Open {
                    open: wire::Open {
                        channel: wire::Channel::Link,
                        lowest: 1,
                        highest: 1,
                        name: copy_of(&link.config.name),
                        secret: copy_of(&link.config.secret),
                    },
                }),
                &mut link.events,
                &mut link.lower,
            );
            link.last_ping = env.now;
            link.machine = Some(machine);
            link.state = State::Opening;
            drain(link, env, out, lower);
        }
        skein_io::Event::Stream { owner, up } => {
            if owner != link.owner || link.state == State::Closing {
                return;
            }
            if let Some(machine) = &mut link.machine {
                let before = machine.received_frames();
                let room = match &up {
                    stream::Up::Room => true,
                    stream::Up::Bytes(_) | stream::Up::End | stream::Up::Failed(_) => false,
                };
                machine::up(machine, &env.limits.channel, &env.limits.sizes, up, &mut link.events, &mut link.lower);
                if machine.received_frames() > before {
                    link.last_heard = env.now;
                }
                if room && machine.queued_bytes() > 0 {
                    link.stalled = Some(env.now.saturating_add(env.limits.channel.stall));
                }
            }
            drain(link, env, out, lower);
        }
        skein_io::Event::Failed { owner, .. } => {
            if owner == link.owner && link.state != State::Idle {
                link.state = State::Closing;
                link.close_sent = true;
                link.handshake = None;
                link.stalled = None;
            }
        }
        skein_io::Event::Closed { owner } => {
            if owner != link.owner || link.state == State::Idle {
                return;
            }
            link.machine = None;
            for _ in 0..machine::MAX_UP {
                let Some(event) = link.events.pop() else {
                    break;
                };
                drop(event);
            }
            for _ in 0..machine::MAX_DOWN {
                let Some(down) = link.lower.pop() else {
                    break;
                };
                drop(down);
            }
            link.read_again = false;
            link.fact_records = 0;
            link.fact_bytes = 0;
            link.socket = None;
            link.state = State::Idle;
            link.handshake = None;
            link.stalled = None;
            out.push(worker::Event::Lost);
        }
        skein_io::Event::Listening { .. }
        | skein_io::Event::Accepted { .. }
        | skein_io::Event::Output { .. }
        | skein_io::Event::Spawned { .. }
        | skein_io::Event::Exited { .. } => {}
    }
}
pub fn resume(
    link: &mut Link,
    env: &Env<Limits>,
    out: &mut Queue<worker::Event>,
    lower: &mut Queue<skein_io::Request>,
) {
    if let Some(machine) = &mut link.machine {
        if link.read_again && link.state == State::Open {
            link.read_again = false;
            machine::down(
                machine,
                &env.limits.channel,
                &env.limits.sizes,
                machine::Request::Read,
                &mut link.events,
                &mut link.lower,
            );
        } else {
            machine::poll(machine, &env.limits.channel, &env.limits.sizes, &mut link.events, &mut link.lower);
        }
    }
    drain(link, env, out, lower);
}
pub fn fire(link: &mut Link, env: &Env<Limits>, out: &mut Queue<worker::Event>, lower: &mut Queue<skein_io::Request>) {
    link.credentials.expire(env.now);
    if let Some(at) = link.handshake
        && at <= env.now
    {
        close(link, env);
        drain(link, env, out, lower);
        return;
    }
    if let Some(at) = link.stalled
        && at <= env.now
    {
        close(link, env);
        drain(link, env, out, lower);
        return;
    }
    if link.state == State::Open {
        if link.last_heard.saturating_add(env.limits.channel.silence) <= env.now {
            close(link, env);
            drain(link, env, out, lower);
            return;
        }
        if link.last_sent.max(link.last_ping).saturating_add(env.limits.channel.ping) <= env.now {
            link.last_ping = env.now;
            let fits = match &link.machine {
                Some(machine) => machine.room() >= 8,
                None => false,
            };
            if fits {
                send(link, env, wire::Message::Ping, out, lower);
            }
        }
    }
}
