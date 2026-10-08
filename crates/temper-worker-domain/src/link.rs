//! The engine link (worker-domain.md, section 2): the one channel the worker
//! keeps open to the engine, which the worker dials, what crosses it while it
//! is down, and the answers until the engine has them.
//!
//! The worker dials at once, and again whenever the channel is lost or a dial
//! fails, after a backoff that doubles with each dial that fails or each
//! channel lost before it has proved itself, from `Limits::redial` up to
//! `Limits::redial_max`, each wait drawn between half the backoff and all of
//! it, so that workers that lost the same engine do not all come back at
//! once. A channel proves itself, and the backoff starts over, once the engine
//! says something on it after the hello. On every channel the worker says
//! hello first: its slots (none once it is shutting down), the workstreams its
//! checkouts hold, and the runs it hosts, which the host reports, with those
//! whose answers the engine has yet to acknowledge.
//!
//! An answer is delivered exactly once, as far as the engine can tell: the
//! worker keeps each until the engine acknowledges it, sends it at once if
//! there is a channel, and again right after every hello, which lists its run
//! as answered so the engine keeps it; the engine drops an answer it has
//! already, acknowledging it again. An answer's slot is not free until the
//! engine has it: the host counts the answers not acknowledged against its
//! slots, so the worker keeps no more than its slots. A refusal holds no slot,
//! and goes once: one lost with the channel reads to the engine as an attempt
//! lost, which it retries as it would a refusal.
//!
//! Losing the channel is survivable. Runs go on for `Limits::grace`, and what
//! they send the engine meanwhile waits: relays in a queue as large as the
//! relays that may wait for the engine at once, and bounces in one as large as
//! the events that may bounce, so neither is dropped. A relay its run's call
//! no longer waits for (withdrawn, or answered as unavailable as the run left
//! live) is dropped from the queue, never sent: an outlet must not happen
//! after its run was told it did not. Past the grace, the worker cancels every
//! run itself, through the host, which saves their work first. A channel that
//! opens again cancels the grace, and the engine keeps or cancels each run
//! the hello lists.
//!
//! A worker shutting down cancels every run, and is done once each has
//! answered and the engine has every answer. It keeps its answers while any
//! run is left, as a channel may yet open: one that does delivers them as
//! usual. Once no run is left, and the channel is still down past the grace,
//! it gives up the answers it keeps instead, and counts them.
//!
//! The transition table. A dial is ended by one `lost`, after a `connected`
//! if the channel opened; every other cell is unreachable by that contract.
//!
//! ```text
//! state     event            next             emits
//! Down      dial alarm       Dialling         dial
//! Dialling  connected        Up, unproved     (the host reports) hello, the answers
//!                                               kept, the relays and bounces; the
//!                                               grace cancelled
//!           lost             Down             (the dial alarm, past the backoff)
//! Up        heard            Up, proved       (the backoff starts over)
//!           lost             Down             (the dial alarm, past the backoff; the grace)
//! any       grace alarm      (same)           (the host cancels every run: contact)
//! ```

use alloc::boxed::Box;

use crate::wire;
use jig_host as host;
use skein_lib::bytes::copy_of;
use skein_lib::{Deadlines, Duration, Env, List, Queue, Rng, Time, Token};
use temper_worker_domain_checkout as checkout;

use crate::boundary::{Hello, Hosted, Request};
use crate::limits::{self, Limits};
use crate::translate;

#[derive(Debug)]
pub(crate) struct Link {
    state: State,
    turns: crate::turns::Turns,
    alarms: Deadlines<Alarm>,
    rng: Rng,
    /// Dials that failed, and channels lost unproved, since a channel last
    /// proved itself.
    failed: u32,
    /// The grace passed since the channel was last open: every run was
    /// cancelled.
    past: bool,
    /// The worker is shutting down.
    shut: bool,
    /// The answers the engine has yet to acknowledge, by the names of their
    /// runs and attempts: sent, or waiting for a channel.
    /// Relays and bounces made while the channel was down, oldest first.
    relays: Queue<Relay>,
    bounces: Queue<Bounced>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum State {
    /// No channel; the dial alarm is armed.
    Down,
    /// A dial is in flight.
    Dialling,
    /// The channel is open, and the hello sent; `proved` once the engine has
    /// said something on it.
    Up { proved: bool },
}

/// The link's alarms.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(crate) enum Alarm {
    /// Dial the engine.
    Dial,
    /// The channel has been down for the grace.
    Grace,
}

/// A relay for the engine, held while the channel is down.
#[derive(PartialEq, Eq, Hash, Debug)]
pub(crate) struct Relay {
    pub(crate) run: Token,
    pub(crate) attempt: Token,
    pub(crate) call: Token,
    pub(crate) body: RelayBody,
}

#[derive(PartialEq, Eq, Hash, Debug)]
pub(crate) enum RelayBody {
    Typed { name: Box<[u8]>, tool: Box<[u8]>, writes: bool, input: Box<[u8]>, deadline: Duration },
}

/// A bounce for the engine, held while the channel is down.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Bounced {
    pub(crate) run: Token,
    pub(crate) attempt: Token,
    pub(crate) name: Token,
    pub(crate) bounce: wire::Bounce,
}

/// What the link's alarm, fired, asks of its parent.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Fired {
    /// It dialled.
    Dialled,
    Turn,
    /// Every run is to be cancelled for contact.
    Grace,
}

impl Link {
    /// A link that dials the engine at once, drawing its jitter from `seed`.
    pub(crate) fn new(limits: &Limits, seed: u64) -> Link {
        let mut alarms = Deadlines::with_capacity(ALARMS);
        alarms.arm(Alarm::Dial, Time::ZERO).expect("room for the link's alarms");
        let bounces = limits::bounces(limits).expect("worst_case accepted the limits");
        Link {
            state: State::Down,
            turns: crate::turns::Turns::new(limits),
            alarms,
            rng: Rng::new(seed),
            failed: 0,
            past: false,
            shut: false,
            relays: Queue::with_capacity(limits.stalled),
            bounces: Queue::with_capacity(bounces),
        }
    }

    pub(crate) const fn is_up(&self) -> bool {
        match self.state {
            State::Up { .. } => true,
            State::Down | State::Dialling => false,
        }
    }

    pub(crate) fn next_deadline(&self) -> Option<Time> {
        match self.alarms.next() {
            Some(at) => match self.turns.next_deadline() {
                Some(next) => Some(at.min(next)),
                None => Some(at),
            },
            None => self.turns.next_deadline(),
        }
    }

    /// Answers the engine has yet to acknowledge.
    pub(crate) fn held(&self) -> u32 {
        self.turns.held()
    }

    /// Relays and bounces waiting for a channel.
    pub(crate) fn stalled(&self) -> u32 {
        self.relays.len().saturating_add(self.bounces.len())
    }

    pub(crate) const fn abandoned(&self) -> u64 {
        self.turns.abandoned()
    }

    pub(crate) const fn is_shut(&self) -> bool {
        self.shut
    }

    /// Whether the link keeps the answer of the run `run`'s attempt `attempt`.
    pub(crate) fn holds(&self, run: Token, attempt: Token) -> bool {
        self.turns.holds(run, attempt)
    }

    /// Fires the link's alarm due at `env.now`, if there is one.
    pub(crate) fn fire(&mut self, env: &Env<Limits>, host: &host::Domain, out: &mut Queue<Request>) -> Option<Fired> {
        let turn_due = match self.turns.next_deadline() {
            Some(at) => {
                at <= env.now
                    && match self.alarms.next() {
                        Some(own) => at <= own,
                        None => true,
                    }
            }
            None => false,
        };
        if turn_due {
            self.turns.fire(env.now, self.is_up(), host, out);
            return Some(Fired::Turn);
        }
        let alarm = self.alarms.expire(env.now)?;
        match alarm {
            Alarm::Dial => {
                assert!(self.state == State::Down, "the dial alarm is armed only while down");
                self.state = State::Dialling;
                out.push(Request::Dial);
                Some(Fired::Dialled)
            }
            Alarm::Grace => {
                self.past = true;
                Some(Fired::Grace)
            }
        }
    }

    /// A reply-bearing assignment cannot enter this link's runtime mode.
    pub(crate) fn refuse_version(&self, run: Token, attempt: Token, out: &mut Queue<Request>) {
        assert!(self.is_up(), "an assignment arrives on an open channel");
        let ending = wire::Ending::Refused(wire::Refusal::Invalid(wire::Invalid::Version));
        out.push(Request::Answer { run, attempt, answer: wire::Answer { turns: 0, spent: 0, ending } });
    }

    /// Refuse an assignment whose application workspace cannot be admitted.
    pub(crate) fn refuse(&self, run: Token, attempt: Token, refusal: wire::Refusal, out: &mut Queue<Request>) {
        assert!(self.is_up(), "an assignment arrives on an open channel");
        let ending = wire::Ending::Refused(refusal);
        out.push(Request::Answer { run, attempt, answer: wire::Answer { turns: 0, spent: 0, ending } });
    }

    /// The channel opened: the hello is next, made once the host reports.
    pub(crate) fn connected_v2(&mut self) {
        assert!(self.state == State::Dialling, "a channel opens once, for a dial in flight");
        self.state = State::Up { proved: false };
        self.past = false;
        self.alarms.cancel(Alarm::Grace);
    }

    /// The engine said something on the channel: it has proved itself, and the
    /// backoff starts over.
    pub(crate) fn heard(&mut self) {
        match self.state {
            State::Up { proved: false } => {
                self.state = State::Up { proved: true };
                self.failed = 0;
            }
            State::Up { proved: true } => {}
            State::Down | State::Dialling => unreachable!("the engine speaks on an open channel"),
        }
    }

    /// The channel closed, or never opened: the worker dials again past the
    /// backoff, and an open channel lost starts the grace.
    pub(crate) fn lost(&mut self, env: &Env<Limits>) {
        let open = match self.state {
            State::Up { .. } => true,
            State::Dialling => false,
            State::Down => unreachable!("a dial is lost once, while it is in flight"),
        };
        self.state = State::Down;
        if open {
            let grace = env.now.saturating_add(env.limits.grace);
            self.alarms.arm(Alarm::Grace, grace).expect("room for the link's alarms");
        }
        let wait = self.backoff(&env.limits);
        self.failed = self.failed.saturating_add(1);
        self.alarms.arm(Alarm::Dial, env.now.saturating_add(wait)).expect("room for the link's alarms");
    }

    /// The worker shuts down: it admits no more runs, and once none is left,
    /// out of reach past the grace, it gives up what it cannot deliver.
    pub(crate) fn shut(&mut self) {
        self.shut = true;
    }

    /// The engine has the answer for the run `run`'s attempt `attempt`. One
    /// the link does not keep was acknowledged already, or was a refusal.
    pub(crate) fn acknowledged(&mut self, run: Token, attempt: Token, host: &host::Domain) {
        self.turns.answer_acknowledged(run, attempt, host);
    }

    /// A relay for the engine: now if the channel is open, kept until it is
    /// otherwise.
    pub(crate) fn relay(&mut self, relay: Relay, host: &host::Domain, out: &mut Queue<Request>) {
        if self.is_up() {
            return out.push(request(relay));
        }
        // Room is made by dropping the relays their calls no longer wait for:
        // those that do are no more than the queue holds, as worst_case
        // checks.
        let relay = match self.relays.try_push(relay) {
            Ok(()) => return,
            Err(relay) => relay,
        };
        for _ in 0..self.relays.capacity() {
            let Some(kept) = self.relays.pop() else {
                break;
            };
            if host.is_relayed(kept.call) {
                self.relays.push(kept);
            }
        }
        self.relays.try_push(relay).expect("room for every relay that waits for the engine");
    }

    /// Cancels local delivery and its wait, without undoing any remote effect.
    /// Queued bytes are released now and return true; a submitted delivery
    /// returns false, so its cancel goes down and its terminal arrives later.
    pub(crate) fn cancel_relay(&mut self, call: Token) -> bool {
        let mut cancelled = false;
        let queued = self.relays.len();
        for _ in 0..queued {
            let relay = self.relays.pop().expect("the queue held this many relays");
            if relay.call == call {
                cancelled = true;
            } else {
                self.relays.push(relay);
            }
        }
        cancelled
    }

    /// A bounce for the engine: now if the channel is open, kept until it is
    /// otherwise.
    pub(crate) fn bounce(&mut self, bounced: Bounced, out: &mut Queue<Request>) {
        let Bounced { run, attempt, name, bounce } = bounced;
        if self.is_up() {
            return out.push(Request::Bounced { run, attempt, name, bounce });
        }
        self.bounces.try_push(bounced).expect("room for every event that may bounce");
    }

    /// The hello, from the host's report `runs`: then the answers kept, and the
    /// relays their calls still wait for and the bounces after them.
    pub(crate) fn hello(
        &mut self,
        runs: &[wire::Hosting],
        host: &host::Domain,
        checkout: &checkout::Domain,
        limits: &Limits,
        out: &mut Queue<Request>,
    ) {
        let count = u32::try_from(runs.len()).expect("the host reports no more runs than its slots");
        let mut hosting = List::with_capacity(count.saturating_add(self.turns.held()));
        for run in runs {
            let hosted = Hosted { run: run.run, attempt: run.attempt, phase: translate::phase(run.phase) };
            hosting.push(hosted).expect("room for every run reported");
        }

        self.turns.hosting(&mut hosting);
        let mut workstreams = List::with_capacity(checkout.workspaces());
        for nth in 0..checkout.workspaces() {
            let Some(key) = checkout.workstream(nth) else {
                break;
            };
            workstreams.push(copy_of(key)).expect("room for every workspace");
        }
        // A worker shutting down takes no more work.
        let slots = if self.shut { 0 } else { limits.host.slots };
        let hello = Hello { slots, workstreams: workstreams.into_boxed(), hosting: hosting.into_boxed() };
        out.push(Request::HelloV2 {
            hello,
            graces: limits::declared_graces(limits).expect("startup checked the stop bound"),
            push_deadline: limits::push_deadline(limits).expect("startup checked push bound"),
        });
        self.turns.hello(host, out);

        let queued = self.relays.len();
        for _ in 0..queued {
            let Some(relay) = self.relays.pop() else {
                break;
            };
            if host.is_relayed(relay.call) {
                out.push(request(relay));
            }
        }
        for _ in 0..self.bounces.capacity() {
            let Some(Bounced { run, attempt, name, bounce }) = self.bounces.pop() else {
                break;
            };
            out.push(Request::Bounced { run, attempt, name, bounce });
        }
    }

    /// A worker shutting down, out of reach past the grace, gives up the
    /// answers it keeps. Its parent calls this once no run is left: no answer
    /// is to come that a channel opening could deliver with the rest.
    pub(crate) fn give_up(&mut self, host: &host::Domain) -> bool {
        if !(self.shut && self.past) {
            return false;
        }
        self.turns.give_up(host);

        true
    }

    pub(crate) fn answer_v2(&mut self, run: Token, attempt: Token, answer: wire::Answer, out: &mut Queue<Request>) {
        self.turns.answer(run, attempt, answer, self.is_up(), out);
    }
    pub(crate) fn turn(&mut self, run: Token, attempt: Token, turn: wire::Turn, out: &mut Queue<Request>) {
        if self.is_up() {
            out.push(Request::Turn { run, attempt, turn });
        }
    }
    pub(crate) fn turn_acknowledged(&mut self, run: Token, attempt: Token, turn: u32, host: &host::Domain) {
        self.turns.acknowledge_turn(run, attempt, turn, host);
    }
    pub(crate) fn turn_busy(&mut self, run: Token, attempt: Token, turn: u32, env: &Env<Limits>, host: &host::Domain) {
        self.turns.busy(run, attempt, turn, env, host);
    }

    /// How long to wait before the next dial: the backoff for the dials failed
    /// so far, jittered down to half of it.
    fn backoff(&mut self, limits: &Limits) -> Duration {
        let doubling = 1_u64.checked_shl(self.failed).unwrap_or(u64::MAX);
        let base = limits.redial.as_nanos().saturating_mul(doubling).min(limits.redial_max.as_nanos());
        Duration::from_nanos(self.rng.between(base.checked_shr(1).unwrap_or(0), base))
    }
}

/// The link's alarms: the dial and the grace.
pub(crate) const ALARMS: u32 = 2;

fn request(relay: Relay) -> Request {
    let Relay { run, attempt, call, body } = relay;
    match body {
        RelayBody::Typed { name, tool, writes, input, deadline } => {
            Request::Relay { run, attempt, call: name, delivery: call, tool, writes, input, deadline }
        }
    }
}
