//! Stream half of a spawned agent. Process creation, signals, waiting and
//! reaping attach here when skein exposes them; no process lifetime is inferred
//! from a frame-machine fault or stdout EOF.
use crate::{Limits, credentials, translate};
use alloc::boxed::Box;
use skein_lib::{Duration, Env, List, Queue, Time, Token, stream};
use temper_channel::{
    codec,
    machine::{self, Endpoint, Machine},
    wire,
};
use temper_worker_domain::{self as worker, agent::channel};

pub const MAX_UP: u32 = 2;
pub const MAX_DOWN: u32 = machine::MAX_DOWN;
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Opening,
    Ready,
    Failed,
    Closed,
}
struct Pending {
    owner: Token,
    message: wire::Message,
    length: u32,
    filled: Time,
}
#[expect(missing_debug_implementations, reason = "pending credential values must never occur in traces")]
pub struct Channel {
    process: Token,
    state: State,
    deadline: Time,
    machine: Machine,
    sending: Option<Pending>,
    sent_owner: Option<Token>,
    reading: Option<Token>,
    malformed: bool,
    events: Queue<machine::Event>,
    lower: Queue<stream::Down>,
}
impl Channel {
    #[must_use]
    pub fn new(process: Token, deadline: Time, limits: &Limits) -> Option<Channel> {
        Some(Channel {
            process,
            deadline,
            state: State::Opening,
            machine: Machine::new(Endpoint::WorkerAgent, &limits.channel, &limits.sizes)?,
            sending: None,
            sent_owner: None,
            reading: None,
            malformed: false,
            events: Queue::with_capacity(machine::MAX_UP),
            lower: Queue::with_capacity(machine::MAX_DOWN),
        })
    }
    #[must_use]
    pub const fn state(&self) -> State {
        self.state
    }
    #[must_use]
    pub const fn process(&self) -> Token {
        self.process
    }
    #[must_use]
    pub fn is_ready(&self) -> bool {
        if self.state == State::Failed || self.state == State::Closed {
            return false;
        }
        !self.lower.is_empty()
            || self.machine.is_ready()
            || match &self.sending {
                Some(send) => {
                    self.state == State::Ready
                        && self.machine.pending_bytes() == 0
                        && send.length <= self.machine.room()
                }
                None => false,
            }
    }
    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        let expiry = match &self.sending {
            Some(send) => expires(send),
            None => None,
        };
        if self.state == State::Opening {
            Some(match expiry {
                Some(at) => at.min(self.deadline),
                None => self.deadline,
            })
        } else {
            expiry
        }
    }
}
/// Start the worker's frozen Open once the real process's pipes exist.
pub fn start(
    channel: &mut Channel,
    env: &Env<Limits>,
    out: &mut Queue<worker::Event>,
    lower: &mut Queue<stream::Down>,
) {
    machine::down(
        &mut channel.machine,
        &env.limits.channel,
        &env.limits.sizes,
        machine::Request::Send(wire::Message::Open {
            open: wire::Open {
                channel: wire::Channel::Agent,
                lowest: 1,
                highest: 1,
                name: Box::new([]),
                secret: Box::new([]),
            },
        }),
        &mut channel.events,
        &mut channel.lower,
    );
    drain(channel, env, out, lower);
}
fn terminal_read(channel: &mut Channel, out: &mut Queue<worker::Event>) {
    if let Some(owner) = channel.reading.take() {
        out.push(if channel.malformed { worker::Event::Malformed { owner } } else { worker::Event::Hangup { owner } });
    }
}
fn fail(channel: &mut Channel, out: &mut Queue<worker::Event>) {
    channel.state = State::Failed;
    if let Some(send) = channel.sending.take() {
        out.push(worker::Event::Unsent { owner: send.owner });
    }
    if let Some(owner) = channel.sent_owner.take() {
        out.push(worker::Event::Unsent { owner });
    }
    terminal_read(channel, out);
}
fn drain(channel: &mut Channel, env: &Env<Limits>, out: &mut Queue<worker::Event>, lower: &mut Queue<stream::Down>) {
    for _ in 0..machine::MAX_DOWN {
        let Some(down) = channel.lower.pop() else {
            break;
        };
        lower.push(down);
    }
    for _ in 0..machine::MAX_UP {
        let Some(event) = channel.events.pop() else {
            break;
        };
        match event {
            machine::Event::Ready { .. } => channel.state = State::Ready,
            machine::Event::Sent => {
                if let Some(owner) = channel.sent_owner.take() {
                    out.push(worker::Event::Sent { owner });
                }
            }
            machine::Event::Unsent => {
                if let Some(owner) = channel.sent_owner.take() {
                    out.push(worker::Event::Unsent { owner });
                }
            }
            machine::Event::Message(message) => {
                if let Some(owner) = channel.reading.take() {
                    match translate::agent::up(message, &env.limits.sizes) {
                        Ok(message) => out.push(worker::Event::Received { owner, message }),
                        Err(
                            translate::Error::Direction
                            | translate::Error::Limits
                            | translate::Error::Credential
                            | translate::Error::Unsupported,
                        ) => {
                            channel.malformed = true;
                            out.push(worker::Event::Malformed { owner });
                            fail(channel, out);
                        }
                    }
                }
            }
            machine::Event::Closed { fault } => {
                channel.malformed = match fault {
                    machine::Fault::Framing | machine::Fault::Limits | machine::Fault::Version => true,
                    machine::Fault::End
                    | machine::Fault::Stream(_)
                    | machine::Fault::Closed
                    | machine::Fault::OutputFull => false,
                };
                fail(channel, out);
            }
            machine::Event::ReadEnded => {}
        }
    }
}
#[derive(Debug)]
pub struct Send {
    pub owner: Token,
    pub message: channel::Down,
}
pub fn send(
    channel: &mut Channel,
    env: &Env<Limits>,
    request: Send,
    endpoints: &[wire::EndpointDescriptor],
    credentials: &credentials::Table,
    out: &mut Queue<worker::Event>,
    lower: &mut Queue<stream::Down>,
) -> Result<(), translate::Error> {
    let Send { owner, message } = request;
    if channel.state == State::Failed || channel.state == State::Closed {
        out.push(worker::Event::Unsent { owner });
        return Ok(());
    }
    assert!(
        channel.sending.is_none() && channel.sent_owner.is_none(),
        "the domain sends one message at a time per agent"
    );
    let message = match translate::agent::down(message, endpoints, credentials, env.now, &env.limits.sizes) {
        Ok(Some(message)) => message,
        Ok(None) => {
            out.push(worker::Event::Sent { owner });
            return Ok(());
        }
        Err(error) => {
            out.push(worker::Event::Unsent { owner });
            return Err(error);
        }
    };
    let length = codec::frame_len(&message, &env.limits.sizes).expect("translation validated");
    channel.sending = Some(Pending { owner, message, length, filled: env.now });
    resume(channel, env, out, lower);
    Ok(())
}
pub fn read(
    channel: &mut Channel,
    env: &Env<Limits>,
    owner: Token,
    out: &mut Queue<worker::Event>,
    lower: &mut Queue<stream::Down>,
) {
    assert!(channel.reading.is_none(), "one agent read waits at a time");
    channel.reading = Some(owner);
    if channel.state == State::Failed || channel.state == State::Closed {
        terminal_read(channel, out);
        return;
    }
    machine::down(
        &mut channel.machine,
        &env.limits.channel,
        &env.limits.sizes,
        machine::Request::Read,
        &mut channel.events,
        &mut channel.lower,
    );
    drain(channel, env, out, lower);
}
pub fn up(
    channel: &mut Channel,
    env: &Env<Limits>,
    event: stream::Up,
    out: &mut Queue<worker::Event>,
    lower: &mut Queue<stream::Down>,
) {
    if channel.state == State::Failed || channel.state == State::Closed {
        return;
    }
    machine::up(
        &mut channel.machine,
        &env.limits.channel,
        &env.limits.sizes,
        event,
        &mut channel.events,
        &mut channel.lower,
    );
    drain(channel, env, out, lower);
}
pub fn resume(
    channel: &mut Channel,
    env: &Env<Limits>,
    out: &mut Queue<worker::Event>,
    lower: &mut Queue<stream::Down>,
) {
    if let Some(send) = &mut channel.sending
        && !age(send, env.now, &env.limits.sizes)
    {
        let owner = channel.sending.take().expect("a retired grant was pending").owner;
        out.push(worker::Event::Sent { owner });
    }
    let fits = match &channel.sending {
        Some(send) => {
            channel.state == State::Ready
                && channel.machine.pending_bytes() == 0
                && send.length <= channel.machine.room()
        }
        None => false,
    };
    if fits {
        let send = channel.sending.take().expect("pending send fits");
        channel.sent_owner = Some(send.owner);
        machine::down(
            &mut channel.machine,
            &env.limits.channel,
            &env.limits.sizes,
            machine::Request::Send(send.message),
            &mut channel.events,
            &mut channel.lower,
        );
    } else {
        machine::poll(
            &mut channel.machine,
            &env.limits.channel,
            &env.limits.sizes,
            &mut channel.events,
            &mut channel.lower,
        );
    }
    drain(channel, env, out, lower);
}
pub fn fire(channel: &mut Channel, env: &Env<Limits>, out: &mut Queue<worker::Event>, lower: &mut Queue<stream::Down>) {
    if channel.state == State::Opening && channel.deadline <= env.now {
        machine::down(
            &mut channel.machine,
            &env.limits.channel,
            &env.limits.sizes,
            machine::Request::Close,
            &mut channel.events,
            &mut channel.lower,
        );
        drain(channel, env, out, lower);
    } else {
        resume(channel, env, out, lower);
    }
}
/// Actual pipe retirement is supplied by the future process/io owner. A frame
/// failure or Finish delivery alone cannot release that binding.
pub fn closed(channel: &mut Channel, out: &mut Queue<worker::Event>) {
    fail(channel, out);
    channel.state = State::Closed;
}

fn expires(send: &Pending) -> Option<Time> {
    let mut expiry: Option<Time> = None;
    match &send.message {
        wire::Message::AgentStart { grants, .. } => {
            for grant in grants {
                let at = send.filled.saturating_add(grant.valid);
                expiry = Some(match expiry {
                    Some(previous) => previous.min(at),
                    None => at,
                });
            }
        }
        wire::Message::AgentGrant { grant } => expiry = Some(send.filled.saturating_add(grant.valid)),
        wire::Message::Open { .. }
        | wire::Message::Accept { .. }
        | wire::Message::Refuse { .. }
        | wire::Message::Ping
        | wire::Message::Terms { .. }
        | wire::Message::Hello { .. }
        | wire::Message::Answer { .. }
        | wire::Message::Relay { .. }
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
        | wire::Message::AgentEvent { .. }
        | wire::Message::AgentAnswer { .. }
        | wire::Message::AgentCancel => {}
    }
    expiry
}
fn age(send: &mut Pending, now: Time, sizes: &temper_channel::Sizes) -> bool {
    let elapsed = now.saturating_since(send.filled).as_nanos();
    if elapsed == 0 {
        return true;
    }
    match &mut send.message {
        wire::Message::AgentStart { grants, .. } => {
            let source = core::mem::replace(grants, Box::new([]));
            let mut kept = List::with_capacity(u32::try_from(source.len()).expect("validated pending grants"));
            for mut grant in source {
                grant.valid = Duration::from_nanos(grant.valid.as_nanos().saturating_sub(elapsed));
                if grant.valid > Duration::ZERO {
                    kept.push(grant).expect("source grant count");
                }
            }
            *grants = kept.into_boxed();
        }
        wire::Message::AgentGrant { grant } => {
            grant.valid = Duration::from_nanos(grant.valid.as_nanos().saturating_sub(elapsed));
            if grant.valid == Duration::ZERO {
                return false;
            }
        }
        wire::Message::Open { .. }
        | wire::Message::Accept { .. }
        | wire::Message::Refuse { .. }
        | wire::Message::Ping
        | wire::Message::Terms { .. }
        | wire::Message::Hello { .. }
        | wire::Message::Answer { .. }
        | wire::Message::Relay { .. }
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
        | wire::Message::AgentEvent { .. }
        | wire::Message::AgentAnswer { .. }
        | wire::Message::AgentCancel => {}
    }
    send.filled = now;
    send.length = codec::frame_len(&send.message, sizes).expect("aging only removes expired values");
    true
}
