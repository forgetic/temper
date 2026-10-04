//! Agent stdin/stdout, through the real channel frame machine (channel.md,
//! 6, 9–10). The owner supplies prepared roots and actual pipe settlement.
use crate::{Error, grants, payload, worker};
use alloc::boxed::Box;
use skein_lib::{Duration, Env, List, Queue, ReplyTo, Time, Token, stream};
use temper_agent_domain::{self as agent, run::charter::Checkout};
use temper_channel::{
    codec,
    machine::{self, Endpoint, Machine},
    wire,
};

/// One frame-machine call and one pending-frame resume; each can emit 2/4.
pub const MAX_UP: u32 = 8;
pub const MAX_DOWN: u32 = machine::MAX_DOWN * 2;
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Limits {
    pub protocol: crate::Limits,
    pub channel: temper_channel::Limits,
    pub sizes: temper_channel::Sizes,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Opening,
    Starting,
    Rooting,
    Running,
    Finishing,
    Failed,
    Closed,
}
#[derive(Debug)]
#[expect(clippy::large_enum_variant, reason = "bounded inline diagnostic tails are counted in queue memory")]
pub enum Up {
    Domain(agent::Event),
    /// Borrow `repositories()` to begin roots preparation; supply them to `roots()`.
    Roots,
    Below(agent::Request),
    Refused(Error),
    Fault(machine::Fault),
    /// The owner must begin lower closure on a fault; binding remains until `closed()`.
    /// Only the real lower owner, through `closed()`, can issue this event.
    Closed,
}
struct Starting {
    charter: agent::run::Charter,
    repositories: Box<[wire::AgentRepository]>,
}
struct Pending {
    message: wire::Message,
    length: u32,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum CheckNotice {
    None,
    Open,
    BeforeLong,
    Done,
    Queued,
}
#[expect(missing_debug_implementations, reason = "channel owns credential values and must never trace them")]
pub struct Channel {
    worker: Token,
    state: State,
    machine: Machine,
    endpoints: Box<[wire::EndpointDescriptor]>,
    credentials: grants::Table,
    starting: Option<Starting>,
    run: Option<Token>,
    call: Option<Token>,
    spent: Option<agent::run::Spend>,
    cancel: bool,
    read_again: bool,
    check_notice: CheckNotice,
    check_operation: Option<Token>,
    sending: Option<Pending>,
    fact_debt: u32,
    fact_bytes: u32,
    opened: Time,
    events: Queue<machine::Event>,
    lower: Queue<stream::Down>,
}
impl Channel {
    /// Pass the domain's actual `limits.skew`: both layers apply the same
    /// skew once, against the original received remaining validity.
    #[must_use]
    pub fn new(worker: Token, now: Time, limits: &Limits, domain_skew: Duration) -> Option<Channel> {
        if limits.protocol.skew != domain_skew || worst_case(limits).is_none() {
            return None;
        }
        Some(Channel {
            worker,
            state: State::Opening,
            machine: Machine::new(Endpoint::Agent, &limits.channel, &limits.sizes)?,
            endpoints: Box::new([]),
            credentials: grants::Table::new(&[], &limits.protocol).ok()?,
            starting: None,
            run: None,
            call: None,
            spent: None,
            cancel: false,
            read_again: false,
            check_notice: CheckNotice::None,
            check_operation: None,
            sending: None,
            fact_debt: 0,
            fact_bytes: 0,
            opened: now,
            events: Queue::with_capacity(machine::MAX_UP),
            lower: Queue::with_capacity(machine::MAX_DOWN),
        })
    }
    #[must_use]
    pub const fn state(&self) -> State {
        self.state
    }
    /// v1 cannot transmit spend: it stays observable to the owner after Finish.
    #[must_use]
    pub const fn spent(&self) -> Option<agent::run::Spend> {
        self.spent
    }
    #[must_use]
    pub fn endpoints(&self) -> &[wire::EndpointDescriptor] {
        &self.endpoints
    }
    #[must_use]
    pub fn credentials(&self) -> &grants::Table {
        &self.credentials
    }
    pub fn credentials_mut(&mut self) -> &mut grants::Table {
        &mut self.credentials
    }
    #[must_use]
    pub fn repositories(&self) -> &[wire::AgentRepository] {
        match &self.starting {
            Some(value) => &value.repositories,
            None => &[],
        }
    }
    /// Admission for worker-bound sends only; io/LLM cleanup remains routable.
    #[must_use]
    pub fn can_send(&self) -> bool {
        self.sending.is_none()
            && self.check_notice != CheckNotice::Done
            && self.check_notice != CheckNotice::BeforeLong
            && self.state == State::Running
    }
    #[must_use]
    pub fn can_take(&self, request: &agent::Request) -> bool {
        if self.state == State::Failed || self.state == State::Closed {
            return true;
        }
        match request {
            agent::Request::Complete { .. }
            | agent::Request::Cancel { .. }
            | agent::Request::Io { .. }
            | agent::Request::CancelIo { .. }
            | agent::Request::Read { .. }
            | agent::Request::Probe { .. }
            | agent::Request::Abort { .. }
            | agent::Request::Admitted { .. } => true,
            agent::Request::Check { .. } => {
                self.check_operation.is_none()
                    && self.check_notice != CheckNotice::Done
                    && self.check_notice != CheckNotice::BeforeLong
            }
            agent::Request::Checking { .. } => {
                self.sending.is_none() && self.state == State::Running && self.check_notice != CheckNotice::Done
            }
            agent::Request::Answer { .. }
            | agent::Request::Push { .. }
            | agent::Request::CancelHost { .. }
            | agent::Request::Rejected { .. }
            | agent::Request::Exhausted { .. } => self.can_send(),
        }
    }
    /// Check before removing a fact from the domain. Fact frames never occupy
    /// the pending slot while blocked or consume owed-message byte/slot reserves.
    #[must_use]
    pub fn can_fact(&self, limits: &Limits) -> bool {
        let Some(frame) = temper_channel::sizes::frame(0x203, &limits.sizes) else {
            return false;
        };
        let Some(reserve) = limits.sizes.facts.checked_mul(frame) else {
            return false;
        };
        let Some(cap) = temper_channel::sizes::output_cap(Endpoint::Agent, &limits.sizes) else {
            return false;
        };
        let Some(owed) = cap.checked_sub(reserve) else {
            return false;
        };
        let Some(bytes) = self.fact_bytes.checked_add(frame) else {
            return false;
        };
        self.can_send()
            && self.fact_debt < limits.sizes.facts
            && bytes <= reserve
            && self.machine.room().saturating_sub(owed) >= frame
    }
    /// Shape facts may be popped into one owner-held slot to inspect this
    /// admission. Count/drop blocked ordinary facts. `CheckFinished` is consumed
    /// metadata; the actual operation owner supplies owed `LongDone` independently.
    #[must_use]
    pub fn can_take_fact(&self, fact: &agent::Fact, limits: &Limits) -> bool {
        match fact {
            agent::Fact::Run { fact } => match fact {
                agent::run::facts::Fact::CheckFinished { .. } => self.state == State::Running,
                agent::run::facts::Fact::Admitted { .. }
                | agent::run::facts::Fact::Prepared { .. }
                | agent::run::facts::Fact::Opened { .. }
                | agent::run::facts::Fact::Ended { .. }
                | agent::run::facts::Fact::Called { .. }
                | agent::run::facts::Fact::Returned { .. }
                | agent::run::facts::Fact::CheckStarted { .. }
                | agent::run::facts::Fact::Pushed { .. }
                | agent::run::facts::Fact::Answered { .. } => self.can_fact(limits),
            },
            agent::Fact::Session { .. } => self.can_fact(limits),
        }
    }
    #[must_use]
    pub fn is_ready(&self) -> bool {
        if self.state == State::Failed || self.state == State::Closed {
            return false;
        }
        let pending = match &self.sending {
            Some(value) => value.length <= self.machine.room(),
            None => false,
        };
        self.read_again
            || (self.check_notice == CheckNotice::Done && self.sending.is_none())
            || pending
            || self.machine.is_ready()
    }
    #[must_use]
    pub fn next_deadline(&self, limits: &Limits) -> Option<Time> {
        if self.state == State::Failed || self.state == State::Closed {
            return None;
        }
        let expiry = self.credentials.next_deadline();
        match self.state {
            State::Opening => Some(match expiry {
                Some(at) => at.min(self.opened.saturating_add(limits.channel.handshake)),
                None => self.opened.saturating_add(limits.channel.handshake),
            }),
            State::Starting | State::Rooting | State::Running | State::Finishing => expiry,
            State::Failed | State::Closed => None,
        }
    }
}
fn queue(channel: &mut Channel, message: wire::Message, sizes: &temper_channel::Sizes) -> Result<(), Error> {
    if channel.sending.is_some() {
        return Err(Error::TooLarge);
    }
    let length = codec::frame_len(&message, sizes).ok_or(Error::TooLarge)?;
    channel.sending = Some(Pending { message, length });
    Ok(())
}
fn cancelled(channel: &mut Channel, out: &mut Queue<Up>) {
    if !channel.cancel {
        channel.cancel = true;
        if let Some(run) = channel.run {
            out.push(Up::Domain(agent::Event::Cancel { run }));
        }
    }
}
fn failed(channel: &mut Channel, fault: machine::Fault, out: &mut Queue<Up>) {
    if channel.state == State::Failed || channel.state == State::Closed {
        return;
    }
    channel.state = State::Failed;
    channel.starting = None;
    channel.sending = None;
    channel.check_notice = CheckNotice::None;
    channel.read_again = false;
    cancelled(channel, out);
    if let Some(owner) = channel.call.take() {
        out.push(Up::Domain(agent::Event::Pushed {
            owner,
            push: agent::run::Push::Failed {
                failure: agent::run::PushFailure::new(agent::run::PushReason::Unavailable),
            },
        }));
    }
    out.push(Up::Fault(fault));
}
fn refuse(channel: &mut Channel, error: Error, sizes: &temper_channel::Sizes, out: &mut Queue<Up>) {
    channel.starting = None;
    channel.state = State::Finishing;
    channel.read_again = false;
    if queue(
        channel,
        wire::Message::Finish { finish: wire::Finish::Failed { failure: wire::RunFailure::Policy } },
        sizes,
    )
    .is_err()
    {
        failed(channel, machine::Fault::OutputFull, out);
    }
    out.push(Up::Refused(error));
}
fn entrance(
    channel: &mut Channel,
    message: wire::Message,
    env: &Env<Limits>,
    out: &mut Queue<Up>,
) -> Result<(), Error> {
    let (charter, snapshot, repositories, endpoints, grants) = match message {
        wire::Message::AgentStart { charter, snapshot, repositories, endpoints, grants } => {
            (charter, snapshot, repositories, endpoints, grants)
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
        | wire::Message::AgentGrant { .. }
        | wire::Message::AgentAnswer { .. }
        | wire::Message::AgentCancel
        | wire::Message::AgentEvent { .. } => return Err(Error::Malformed),
    };
    worker::snapshot(&snapshot)?;
    for (index, repository) in repositories.iter().enumerate() {
        if repository.name.is_empty() || repository.name.contains(&0) {
            return Err(Error::Malformed);
        }
        for other in repositories.get(..index).ok_or(Error::Malformed)? {
            if repository.name == other.name {
                return Err(Error::Malformed);
            }
        }
    }
    let charter = payload::charter(
        &charter,
        Checkout { repositories: Box::new([]) },
        &endpoints,
        &env.limits.sizes,
        &env.limits.protocol,
    )?;
    let scope = payload::scope(&charter, &env.limits.protocol)?;
    let mut table = grants::Table::new(&scope, &env.limits.protocol)?;
    for (index, grant) in grants.iter().enumerate() {
        for other in grants.get(..index).ok_or(Error::Malformed)? {
            if grant.account == other.account {
                return Err(Error::Grant);
            }
        }
    }
    for grant in grants {
        table.insert(grant, env.now, &env.limits.protocol)?;
    }
    channel.credentials = table;
    channel.endpoints = endpoints;
    channel.starting = Some(Starting { charter, repositories });
    channel.state = State::Rooting;
    out.push(Up::Roots);
    Ok(())
}
fn message(channel: &mut Channel, value: wire::Message, env: &Env<Limits>, out: &mut Queue<Up>) {
    if channel.state == State::Starting {
        if let Err(error) = entrance(channel, value, env, out) {
            refuse(channel, error, &env.limits.sizes, out);
        }
        return;
    }
    match value {
        wire::Message::AgentGrant { grant } => match channel.credentials.insert(grant, env.now, &env.limits.protocol) {
            Ok(Some(grant)) if channel.state == State::Running => out.push(Up::Domain(agent::Event::Grant { grant })),
            Ok(_) => {}
            Err(error) => {
                out.push(Up::Refused(error));
                failed(channel, machine::Fault::Framing, out);
            }
        },
        wire::Message::AgentCancel => cancelled(channel, out),
        wire::Message::AgentAnswer { call, reply } => {
            if let Some(owner) = channel.call
                && owner.raw() == call
            {
                match worker::answer(owner, reply) {
                    Ok(event) => {
                        channel.call = None;
                        out.push(Up::Domain(event));
                    }
                    Err(error) => {
                        out.push(Up::Refused(error));
                        failed(channel, machine::Fault::Framing, out);
                    }
                }
            }
        }
        // The current run has no inbound-event operation; retain no bytes and
        // do not invent a policy for the future long-lived agent domain.
        wire::Message::AgentEvent { .. } => {}
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
        | wire::Message::AgentStart { .. } => failed(channel, machine::Fault::Framing, out),
    }
}
fn drain(channel: &mut Channel, env: &Env<Limits>, out: &mut Queue<Up>, lower: &mut Queue<stream::Down>) {
    for _ in 0..machine::MAX_DOWN {
        let Some(value) = channel.lower.pop() else {
            break;
        };
        lower.push(value);
    }
    for _ in 0..machine::MAX_UP {
        let Some(value) = channel.events.pop() else {
            break;
        };
        match value {
            machine::Event::Ready { .. } => channel.state = State::Starting,
            machine::Event::Message(value) => {
                message(channel, value, env, out);
                if channel.state == State::Rooting || channel.state == State::Running {
                    channel.read_again = true;
                }
            }
            machine::Event::Sent => {}
            machine::Event::Unsent => failed(channel, machine::Fault::OutputFull, out),
            machine::Event::ReadEnded => {
                channel.read_again = false;
                cancelled(channel, out);
            }
            machine::Event::Closed { fault } => failed(channel, fault, out),
        }
    }
}
pub fn resume(channel: &mut Channel, env: &Env<Limits>, out: &mut Queue<Up>, lower: &mut Queue<stream::Down>) {
    if channel.state == State::Failed || channel.state == State::Closed {
        return;
    }
    channel.credentials.expire(env.now);
    if channel.check_notice == CheckNotice::Done && channel.sending.is_none() && channel.state == State::Running {
        // This fixed owed-notice slot orders settlement before a subsequent
        // Checking/Finish even if a best-effort CheckFinished fact was dropped.
        queue(channel, wire::Message::LongDone, &env.limits.sizes).expect("fixed v1 LongDone fits");
        channel.check_notice = CheckNotice::Queued;
    }
    let cap = temper_channel::sizes::output_cap(Endpoint::Agent, &env.limits.sizes).expect("machine admitted sizes");
    if channel.sending.is_none() && channel.machine.room() == cap && channel.machine.queued_bytes() == 0 {
        channel.fact_debt = 0;
        channel.fact_bytes = 0;
    }
    let fits = match &channel.sending {
        Some(value) => value.length <= channel.machine.room(),
        None => false,
    };
    if channel.read_again {
        channel.read_again = false;
        machine::down(
            &mut channel.machine,
            &env.limits.channel,
            &env.limits.sizes,
            machine::Request::Read,
            &mut channel.events,
            &mut channel.lower,
        );
    } else if fits {
        let send = channel.sending.take().expect("pending frame fits");
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
pub fn up(
    channel: &mut Channel,
    env: &Env<Limits>,
    event: stream::Up,
    out: &mut Queue<Up>,
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
    resume(channel, env, out, lower);
}
/// The roots owner supplies real opaque root tokens. A mismatch fails policy.
pub fn roots(channel: &mut Channel, env: &Env<Limits>, checkout: Checkout, out: &mut Queue<Up>) -> Result<(), Error> {
    let Some(mut start) = channel.starting.take() else {
        return Err(Error::Malformed);
    };
    let mut valid = start.repositories.len() == checkout.repositories.len();
    for (expected, actual) in start.repositories.iter().zip(checkout.repositories.iter()) {
        valid &= expected.name == actual.name && expected.writable == actual.writable;
    }
    if !valid {
        refuse(channel, Error::Malformed, &env.limits.sizes, out);
        return Err(Error::Malformed);
    }
    start.charter.checkout = checkout;
    channel.state = State::Running;
    out.push(Up::Domain(agent::Event::Start {
        reply_to: ReplyTo::new(channel.worker),
        worker: channel.worker,
        charter: start.charter,
        grants: channel.credentials.names(env.now),
    }));
    Ok(())
}
/// The owner drains facts before requests. Fact admission has its own debt:
/// count and bytes remain reserved for owed domain messages until full Room.
pub fn fact(channel: &mut Channel, env: &Env<Limits>, fact: agent::Fact) -> Result<bool, Error> {
    if !channel.can_take_fact(&fact, &env.limits) {
        return Ok(false);
    }
    let message = worker::fact(fact, &env.limits.sizes)?;
    // LongDone ends an owed Long and uses that reserve, rather than fact debt.
    let is_fact = message.kind() == 0x203;
    // This shape fact may have been delayed, dropped or duplicated. Only the
    // fenced actual operation-owner hook is allowed to settle a watchdog pause.
    if !is_fact {
        return Ok(true);
    }
    if is_fact && !channel.can_fact(&env.limits) {
        return Ok(false);
    }
    let length = codec::frame_len(&message, &env.limits.sizes).ok_or(Error::TooLarge)?;
    queue(channel, message, &env.limits.sizes)?;
    if is_fact {
        channel.fact_debt = channel.fact_debt.checked_add(1).expect("bounded by facts");
        channel.fact_bytes = channel.fact_bytes.checked_add(length).expect("bounded fact reserve");
    }
    Ok(true)
}
pub fn content(channel: &mut Channel, env: &Env<Limits>, value: &agent::Content) -> Result<bool, Error> {
    if !channel.can_fact(&env.limits) {
        return Ok(false);
    }
    let message = worker::content(value, &env.limits.sizes)?;
    let length = codec::frame_len(&message, &env.limits.sizes).ok_or(Error::TooLarge)?;
    queue(channel, message, &env.limits.sizes)?;
    channel.fact_debt = channel.fact_debt.checked_add(1).expect("bounded by facts");
    channel.fact_bytes = channel.fact_bytes.checked_add(length).expect("bounded fact reserve");
    Ok(true)
}
/// Bind the fresh actual operation token, after a Check was routed Below. The
/// future io owner issues generation-bearing handles in its own namespace;
/// these are distinct from reusable domain Call owners (programming-model.md, 4.2).
pub fn begin_check(channel: &mut Channel, operation: Token) -> Result<bool, Error> {
    if channel.state != State::Running {
        return Ok(false);
    }
    if channel.check_operation.is_some()
        || channel.check_notice == CheckNotice::Done
        || channel.check_notice == CheckNotice::BeforeLong
    {
        return Err(Error::Malformed);
    }
    channel.check_operation = Some(operation);
    Ok(true)
}
/// Actual Check/Abort settlement hook, called before the domain's terminal.
/// Matching/taking this binding fences delayed and duplicate lower terminals;
/// one fixed owed notice orders settlement before another Long/Finish. Real
/// process/io hookup remains unavailable; droppable facts cannot replace it.
#[must_use]
pub fn end_check(channel: &mut Channel, operation: Token) -> bool {
    if channel.check_operation != Some(operation) {
        return false;
    }
    channel.check_operation = None;
    if channel.state == State::Running {
        channel.check_notice = match channel.check_notice {
            CheckNotice::None => CheckNotice::BeforeLong,
            CheckNotice::Open => CheckNotice::Done,
            CheckNotice::BeforeLong | CheckNotice::Done | CheckNotice::Queued => channel.check_notice,
        };
    }
    true
}
/// One retained outbound domain message. Other io and LLM requests move to the
/// owner unchanged. The owner waits for `can_send()` before removing a request.
pub fn down(
    channel: &mut Channel,
    env: &Env<Limits>,
    request: agent::Request,
    out: &mut Queue<Up>,
) -> Result<(), Error> {
    if !channel.can_take(&request) {
        return Err(Error::TooLarge);
    }
    if channel.state == State::Failed || channel.state == State::Closed {
        return down_closed(channel, request, out);
    }
    let value = match request {
        agent::Request::Admitted { worker, run } => {
            if worker != channel.worker || channel.run.is_some() {
                return Err(Error::Malformed);
            }
            channel.run = Some(run);
            if channel.cancel {
                out.push(Up::Domain(agent::Event::Cancel { run }));
            }
            return Ok(());
        }
        agent::Request::Answer { to, answer } => {
            if to.into_token() != channel.worker {
                return Err(Error::Malformed);
            }
            channel.spent = worker::spent(&answer);
            let projection = worker::finish(answer, &env.limits.sizes)?;
            channel.spent = Some(projection.spent);
            channel.state = State::Finishing;
            channel.read_again = false;
            wire::Message::Finish { finish: projection.frame }
        }
        agent::Request::Checking { worker, deadline } => {
            if worker != channel.worker {
                return Err(Error::Malformed);
            }
            channel.check_notice = match channel.check_notice {
                CheckNotice::BeforeLong => CheckNotice::Done,
                CheckNotice::None | CheckNotice::Open | CheckNotice::Done | CheckNotice::Queued => CheckNotice::Open,
            };
            wire::Message::Long { span: deadline.saturating_since(env.now) }
        }
        agent::Request::Push { worker, owner, change } => {
            if worker != channel.worker || channel.call.is_some() {
                return Err(Error::Malformed);
            }
            let message = payload::message(&change, env.limits.sizes.detail)?;
            channel.call = Some(owner);
            wire::Message::AgentCall { call: owner.raw(), ask: wire::Ask::Push { message } }
        }
        agent::Request::CancelHost { owner } => {
            if channel.call != Some(owner) {
                return Ok(());
            }
            wire::Message::Withdraw { call: owner.raw() }
        }
        agent::Request::Rejected { grant } => {
            wire::Message::AgentRejected { account: grant.account, generation: grant.generation }
        }
        agent::Request::Exhausted { account, retry_after } => wire::Message::AgentExhausted { account, retry_after },
        below @ (agent::Request::Complete { .. }
        | agent::Request::Cancel { .. }
        | agent::Request::Io { .. }
        | agent::Request::CancelIo { .. }
        | agent::Request::Read { .. }
        | agent::Request::Probe { .. }
        | agent::Request::Check { .. }
        | agent::Request::Abort { .. }) => {
            return local_request(channel, below, out);
        }
    };
    queue(channel, value, &env.limits.sizes)
}
fn down_closed(channel: &mut Channel, request: agent::Request, out: &mut Queue<Up>) -> Result<(), Error> {
    match request {
        agent::Request::Answer { to, answer } => {
            if to.into_token() != channel.worker {
                return Err(Error::Malformed);
            }
            channel.spent = Some(match answer {
                agent::run::Answer::Accepted { spent, .. } | agent::run::Answer::Failed { spent, .. } => spent,
                agent::run::Answer::Refused(agent::run::Refusal::Busy | agent::run::Refusal::Invalid(_)) => {
                    return Err(Error::Unsupported);
                }
            });
            Ok(())
        }
        agent::Request::Push { owner, .. } => {
            out.push(Up::Domain(agent::Event::Pushed {
                owner,
                push: agent::run::Push::Failed {
                    failure: agent::run::PushFailure::new(agent::run::PushReason::Unavailable),
                },
            }));
            Ok(())
        }
        agent::Request::Checking { .. }
        | agent::Request::CancelHost { .. }
        | agent::Request::Rejected { .. }
        | agent::Request::Exhausted { .. } => Ok(()),
        local @ (agent::Request::Admitted { .. }
        | agent::Request::Complete { .. }
        | agent::Request::Cancel { .. }
        | agent::Request::Io { .. }
        | agent::Request::CancelIo { .. }
        | agent::Request::Read { .. }
        | agent::Request::Probe { .. }
        | agent::Request::Check { .. }
        | agent::Request::Abort { .. }) => local_request(channel, local, out),
    }
}
fn local_request(channel: &mut Channel, request: agent::Request, out: &mut Queue<Up>) -> Result<(), Error> {
    match request {
        agent::Request::Admitted { worker, run } => {
            if worker != channel.worker || channel.run.is_some() {
                return Err(Error::Malformed);
            }
            channel.run = Some(run);
            if channel.cancel {
                out.push(Up::Domain(agent::Event::Cancel { run }));
            }
            Ok(())
        }
        below @ agent::Request::Check { .. } => {
            // A fresh actual operation can reuse the domain Call owner. Its
            // lower handle fences stale terminals; start a fresh notice cycle.
            if channel.check_notice == CheckNotice::Queued {
                channel.check_notice = CheckNotice::None;
            }
            out.push(Up::Below(below));
            Ok(())
        }
        below @ (agent::Request::Complete { .. }
        | agent::Request::Cancel { .. }
        | agent::Request::Io { .. }
        | agent::Request::CancelIo { .. }
        | agent::Request::Read { .. }
        | agent::Request::Probe { .. }
        | agent::Request::Abort { .. }) => {
            out.push(Up::Below(below));
            Ok(())
        }
        agent::Request::Answer { .. }
        | agent::Request::Checking { .. }
        | agent::Request::Push { .. }
        | agent::Request::CancelHost { .. }
        | agent::Request::Rejected { .. }
        | agent::Request::Exhausted { .. } => Err(Error::Malformed),
    }
}
pub fn close(channel: &mut Channel, env: &Env<Limits>, out: &mut Queue<Up>, lower: &mut Queue<stream::Down>) {
    if channel.state == State::Failed || channel.state == State::Closed {
        return;
    }
    machine::down(
        &mut channel.machine,
        &env.limits.channel,
        &env.limits.sizes,
        machine::Request::Close,
        &mut channel.events,
        &mut channel.lower,
    );
    drain(channel, env, out, lower);
}
pub fn fire(channel: &mut Channel, env: &Env<Limits>, out: &mut Queue<Up>, lower: &mut Queue<stream::Down>) {
    channel.credentials.expire(env.now);
    match channel.state {
        State::Opening if channel.opened.saturating_add(env.limits.channel.handshake) <= env.now => {
            close(channel, env, out, lower);
        }
        State::Opening | State::Starting | State::Rooting | State::Running | State::Finishing => {
            resume(channel, env, out, lower);
        }
        State::Failed | State::Closed => {}
    }
}
/// Called exactly on actual lower settlement, even after a frame fault/EOF.
pub fn closed(channel: &mut Channel, out: &mut Queue<Up>) {
    if channel.state == State::Closed {
        return;
    }
    if channel.state != State::Finishing {
        failed(channel, machine::Fault::Closed, out);
    }
    channel.starting = None;
    channel.sending = None;
    channel.credentials.clear();
    channel.state = State::Closed;
    out.push(Up::Closed);
}

/// Channel state, frame intake/output, retained endpoint/value tables,
/// startup conversion and roots, pending frame/encoded overlap, and scratch
/// plus emitted queues. Allocator overhead is excluded (programming-model.md, 6).
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    if !limits.channel.valid() || !limits.sizes.valid() {
        return None;
    }
    let p = &limits.protocol;
    let s = &limits.sizes;
    let mut frame = 0_u32;
    for kind in [0x201, 0x202, 0x203, 0x204, 0x205, 0x206, 0x207, 0x208, 0x209] {
        frame = frame.max(temper_channel::sizes::frame(kind, s)?);
    }
    let endpoint = List::<wire::EndpointDescriptor>::worst_case(p.endpoints)?
        .checked_add(u64::from(p.endpoints).checked_mul(3)?.checked_mul(u64::from(p.name_bytes))?)?;
    let repositories = List::<wire::AgentRepository>::worst_case(s.repositories)?
        .checked_add(List::<agent::run::charter::Repository>::worst_case(s.repositories)?)?
        .checked_add(u64::from(s.repositories).checked_mul(u64::from(s.name_bytes))?.checked_mul(2)?)?;
    let charter = List::<agent::run::charter::Llm>::worst_case(s.endpoints)?
        .checked_mul(2)?
        .checked_add(List::<temper_channel::payload::Section>::worst_case(s.entries)?)?
        .checked_add(List::<temper_channel::payload::Model>::worst_case(s.endpoints)?)?
        .checked_add(u64::from(s.endpoints).checked_mul(u64::from(s.name_bytes))?)?
        .checked_add(u64::from(p.request_bytes).checked_mul(2)?)?
        .checked_add(u64::from(s.charter))?
        // The fixed report/review rule arrays and their eight short labels.
        .checked_add(List::<agent::run::outcome::VerdictRule>::worst_case(2)?)?
        .checked_add(List::<Box<[u8]>>::worst_case(8)?)?
        .checked_add(128)?;
    let scope = List::<u32>::worst_case(p.accounts)?
        .checked_mul(2)?
        .checked_add(List::<agent::Grant>::worst_case(p.accounts)?.checked_mul(2)?)?;
    let scratch = Queue::<machine::Event>::worst_case(machine::MAX_UP)?
        .checked_add(Queue::<stream::Down>::worst_case(machine::MAX_DOWN)?)?
        .checked_add(Queue::<Up>::worst_case(MAX_UP)?)?
        .checked_add(Queue::<stream::Down>::worst_case(MAX_DOWN)?)?;
    u64::try_from(size_of::<Channel>())
        .ok()?
        .checked_add(temper_channel::sizes::worst_case(Endpoint::Agent, &limits.channel, s)?)?
        .checked_add(endpoint)?
        .checked_add(grants::worst_case(p)?.checked_mul(2)?)?
        .checked_add(repositories)?
        .checked_add(charter)?
        .checked_add(scope)?
        .checked_add(u64::from(frame).checked_mul(2)?)?
        .checked_add(u64::from(s.fact).checked_mul(2)?)?
        .checked_add(scratch)
}
