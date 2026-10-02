//! The engine link (worker-model.md, section 2): the one channel the worker
//! keeps open to the engine, which the worker dials, and what crosses it while
//! it is down.
//!
//! The worker dials at once, and again whenever the channel is lost or a dial
//! fails, after a backoff that doubles with each failed dial, from
//! `Limits::redial` up to `Limits::redial_max`, each wait drawn between half
//! the backoff and all of it, so that workers that lost the same engine do
//! not all come back at once. On every channel it says hello first: its slots,
//! the workstreams its checkouts hold, and the runs it hosts, which the host
//! reports, with those whose answers it holds.
//!
//! Losing the channel is survivable. Runs go on for `Limits::grace`, and what
//! they send the engine meanwhile waits: an answer is held, at most one for
//! each slot (no run is admitted without a channel), and goes right after the
//! next hello, which lists its run as answered so the engine keeps it; relays
//! and bounces wait in a bounded queue and follow the answers, and what does
//! not fit is dropped and counted (a relay dropped is withdrawn by its run
//! once the run's own deadline for it passes, and answered then). Past the
//! grace, the worker cancels every run itself, through the host, which saves
//! their work first; their answers are held like any other. A channel that
//! opens again cancels the grace, and the engine keeps or cancels each run
//! the hello lists.
//!
//! A worker shutting down cancels every run, and is done once each has
//! answered and every answer has gone; while the channel is down past the
//! grace, an answer it holds or makes is given up instead, and counted.
//!
//! The transition table. A dial is ended by one `lost`, after a `connected`
//! if the channel opened; every other cell is unreachable by that contract.
//!
//! ```text
//! state     event         next      emits
//! Down      dial alarm    Dialling  dial
//! Dialling  connected     Up        (the host reports) hello, the held answers,
//!                                     relays and bounces; the grace cancelled
//!           lost          Down      (the dial alarm, past the backoff)
//! Up        lost          Down      (the dial alarm, past the backoff; the grace)
//! any       grace alarm   (same)    (the host cancels every run: contact)
//! ```

use alloc::boxed::Box;

use temper_lib::{Deadlines, Duration, Env, List, Queue, Rng, Time, Token};
use temper_worker_model_checkout as checkout;
use temper_worker_model_host as host;

use crate::boundary::{Hello, Hosted, Phase, Request};
use crate::limits::Limits;
use crate::translate;

#[derive(Debug)]
pub(crate) struct Link {
    state: State,
    alarms: Deadlines<Alarm>,
    rng: Rng,
    /// Dials that failed since the channel was last open.
    failed: u32,
    /// The grace passed since the channel was last open: every run was
    /// cancelled.
    past: bool,
    /// The worker is shutting down.
    shut: bool,
    /// Answers made while the channel was down, oldest first.
    held: Queue<Held>,
    /// Relays and bounces made while the channel was down, oldest first.
    stalled: Queue<Stalled>,
    /// Relays and bounces dropped for want of room, and answers given up.
    dropped: u64,
    abandoned: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum State {
    /// No channel; the dial alarm is armed.
    Down,
    /// A dial is in flight.
    Dialling,
    /// The channel is open, and the hello sent.
    Up,
}

/// The link's alarms.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(crate) enum Alarm {
    /// Dial the engine.
    Dial,
    /// The channel has been down for the grace.
    Grace,
}

/// An answer for the engine, held while the channel is down.
#[derive(PartialEq, Eq, Hash, Debug)]
pub(crate) struct Held {
    run: Token,
    attempt: Token,
    answer: host::Answer,
}

/// A relay or a bounce for the engine, held while the channel is down.
#[derive(PartialEq, Eq, Hash, Debug)]
pub(crate) enum Stalled {
    Relay { run: Token, attempt: Token, call: Token, body: Box<[u8]> },
    Bounced { run: Token, attempt: Token, bounce: host::Bounce },
}

/// What the link's alarm, fired, asks of its parent.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Fired {
    /// It dialled.
    Dialled,
    /// Every run is to be cancelled for contact.
    Grace,
}

impl Link {
    /// A link that dials the engine at once, drawing its jitter from `seed`.
    pub(crate) fn new(limits: &Limits, seed: u64) -> Link {
        let mut alarms = Deadlines::with_capacity(ALARMS);
        alarms.arm(Alarm::Dial, Time::ZERO).expect("room for the link's alarms");
        Link {
            state: State::Down,
            alarms,
            rng: Rng::new(seed),
            failed: 0,
            past: false,
            shut: false,
            held: Queue::with_capacity(limits.host.slots),
            stalled: Queue::with_capacity(limits.stalled),
            dropped: 0,
            abandoned: 0,
        }
    }

    pub(crate) fn is_up(&self) -> bool {
        match self.state {
            State::Up => true,
            State::Down | State::Dialling => false,
        }
    }

    pub(crate) fn next_deadline(&self) -> Option<Time> {
        self.alarms.next()
    }

    pub(crate) fn held(&self) -> u32 {
        self.held.len()
    }

    pub(crate) fn stalled(&self) -> u32 {
        self.stalled.len()
    }

    pub(crate) const fn dropped(&self) -> u64 {
        self.dropped
    }

    pub(crate) const fn abandoned(&self) -> u64 {
        self.abandoned
    }

    pub(crate) const fn is_shut(&self) -> bool {
        self.shut
    }

    /// Fires the link's alarm due at `env.now`, if there is one.
    pub(crate) fn fire(&mut self, env: &Env<Limits>, out: &mut Queue<Request>) -> Option<Fired> {
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
                self.give_up();
                Some(Fired::Grace)
            }
        }
    }

    /// The channel opened: the hello is next, made once the host reports.
    pub(crate) fn connected(&mut self) {
        assert!(self.state == State::Dialling, "a channel opens once, for a dial in flight");
        self.state = State::Up;
        self.failed = 0;
        self.past = false;
        self.alarms.cancel(Alarm::Grace);
    }

    /// The channel closed, or never opened: the worker dials again past the
    /// backoff, and an open channel lost starts the grace.
    pub(crate) fn lost(&mut self, env: &Env<Limits>) {
        let open = match self.state {
            State::Up => true,
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

    /// The worker shuts down: once past the grace, what it cannot deliver is
    /// given up.
    pub(crate) fn shut(&mut self) {
        self.shut = true;
        self.give_up();
    }

    /// The answer for the run `run`'s attempt `attempt`: to the engine now if
    /// the channel is open, held until it is otherwise, or given up by a
    /// worker shutting down past the grace.
    pub(crate) fn answer(&mut self, run: Token, attempt: Token, answer: host::Answer, out: &mut Queue<Request>) {
        if self.is_up() {
            out.push(Request::Answer { run, attempt, answer });
        } else if self.shut && self.past {
            self.abandoned = self.abandoned.saturating_add(1);
        } else {
            // No run is admitted without a channel: one answer at most for
            // each slot.
            self.held.try_push(Held { run, attempt, answer }).expect("room for an answer of every slot");
        }
    }

    /// A relay or a bounce for the engine: now if the channel is open, held
    /// otherwise while there is room, dropped and counted when there is none.
    pub(crate) fn stall(&mut self, stalled: Stalled, out: &mut Queue<Request>) {
        if self.is_up() {
            out.push(request(stalled));
        } else if self.stalled.try_push(stalled).is_err() {
            self.dropped = self.dropped.saturating_add(1);
        }
    }

    /// The hello, from the host's report `runs`: then the answers held, and
    /// the relays and bounces after them.
    pub(crate) fn hello(
        &mut self,
        runs: &[host::Hosting],
        checkout: &checkout::Model,
        limits: &Limits,
        out: &mut Queue<Request>,
    ) {
        let count = u32::try_from(runs.len()).expect("the host reports no more runs than its slots");
        let mut hosting = List::with_capacity(count.saturating_add(self.held.len()));
        for run in runs {
            let hosted = Hosted { run: run.run, attempt: run.attempt, phase: translate::phase(run.phase) };
            hosting.push(hosted).expect("room for every run reported");
        }
        for held in &self.held {
            let hosted = Hosted { run: held.run, attempt: held.attempt, phase: Phase::Answered };
            hosting.push(hosted).expect("room for every answer held");
        }
        let mut workstreams = List::with_capacity(checkout.workspaces());
        for nth in 0..checkout.workspaces() {
            let Some(key) = checkout.workstream(nth) else {
                break;
            };
            workstreams.push(Box::from(key)).expect("room for every workspace");
        }
        let hello =
            Hello { slots: limits.host.slots, workstreams: workstreams.into_boxed(), hosting: hosting.into_boxed() };
        out.push(Request::Hello { hello });
        for _ in 0..self.held.capacity() {
            let Some(Held { run, attempt, answer }) = self.held.pop() else {
                break;
            };
            out.push(Request::Answer { run, attempt, answer });
        }
        for _ in 0..self.stalled.capacity() {
            let Some(stalled) = self.stalled.pop() else {
                break;
            };
            out.push(request(stalled));
        }
    }

    /// A worker shutting down past the grace gives up the answers it holds.
    fn give_up(&mut self) {
        if !(self.shut && self.past) {
            return;
        }
        for _ in 0..self.held.capacity() {
            if self.held.pop().is_none() {
                break;
            }
            self.abandoned = self.abandoned.saturating_add(1);
        }
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

fn request(stalled: Stalled) -> Request {
    match stalled {
        Stalled::Relay { run, attempt, call, body } => Request::Relay { run, attempt, call, body },
        Stalled::Bounced { run, attempt, bounce } => Request::Bounced { run, attempt, bounce },
    }
}
