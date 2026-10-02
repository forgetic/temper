//! The fleet's state and its entry points (programming-style.md, section 3).

use temper_lib::{Deadlines, Env, Id, Map, Queue, Slab, Time, Token};

use crate::attempt::{self, Attempt, Run};
use crate::boundary::{Event, Request};
use crate::call::{self, Call};
use crate::channel::{self, Channel};
use crate::facts::{Fact, Facts};
use crate::limits::Limits;

/// The most requests a step, an alarm or a resume emits under `limits`: a
/// request for each run a hello lists (a cancel again, or the parent told an
/// attempt is found); or an adoption's three (the replaced claim's cancel and
/// its withdrawal, and the adopted attempt's placing or answer). The parent
/// reserves this much room in `out` before calling it.
#[must_use]
pub const fn max_out(limits: &Limits) -> u32 {
    if limits.slots > 3 { limits.slots } else { 3 }
}

/// The fleet's state.
#[derive(Debug)]
pub struct Model {
    /// The workers in contact, and their channels' names.
    pub(crate) channels: Slab<Channel>,
    pub(crate) tokens: Map<Token, Id<Channel>>,
    /// The attempts tracked, by their run's and their own names.
    pub(crate) attempts: Slab<Attempt>,
    pub(crate) names: Map<(Token, Token), Id<Attempt>>,
    /// The runs of the attempts tracked.
    pub(crate) runs: Map<Token, Run>,
    /// The attempts waiting for a slot, in the order they started, and the
    /// count that orders them.
    pub(crate) waiting: Map<u64, Id<Attempt>>,
    pub(crate) serial: u64,
    /// An alarm per attempt, as its state says.
    pub(crate) alarms: Deadlines<Id<Attempt>>,
    /// Relayed calls the parent has yet to answer.
    pub(crate) calls: Slab<Call>,
    /// Something changed that may let a waiting attempt be placed: the ready
    /// list.
    pub(crate) placing: bool,
    pub(crate) facts: Facts,
}

impl Model {
    /// A model with room for `limits`.
    #[must_use]
    pub fn new(limits: &Limits) -> Model {
        Model {
            channels: Slab::with_capacity(limits.workers),
            tokens: Map::with_capacity(limits.workers),
            attempts: Slab::with_capacity(limits.attempts),
            names: Map::with_capacity(limits.attempts),
            runs: Map::with_capacity(limits.attempts),
            waiting: Map::with_capacity(limits.attempts),
            serial: 0,
            alarms: Deadlines::with_capacity(limits.attempts),
            calls: Slab::with_capacity(limits.calls),
            placing: false,
            facts: Facts::with_capacity(limits.facts),
        }
    }

    /// Workers in contact.
    #[must_use]
    pub fn workers(&self) -> u32 {
        self.tokens.len()
    }

    /// Attempts tracked, closed ones included until they are reclaimed.
    #[must_use]
    pub fn attempts(&self) -> u32 {
        self.attempts.len()
    }

    /// Attempts waiting for a slot.
    #[must_use]
    pub fn waiting(&self) -> u32 {
        self.waiting.len()
    }

    /// Relayed calls the parent has yet to answer, answered ones included
    /// until they are reclaimed.
    #[must_use]
    pub fn calls(&self) -> u32 {
        self.calls.len()
    }

    /// When the earliest alarm falls due.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        self.alarms.next()
    }

    /// Whether an alarm is due at `now`. While one is, the loop fires the
    /// top-level model, which calls [`fire`].
    #[must_use]
    pub fn is_due(&self, now: Time) -> bool {
        match self.alarms.next() {
            Some(at) => at <= now,
            None => false,
        }
    }

    /// Whether placement may find a slot for a waiting attempt. While it may,
    /// the loop resumes the top-level model, which calls [`resume`], at the
    /// start of the model's stage, before its input events.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.placing
    }

    /// The oldest fact not yet drained. The parent drains them at its own
    /// pace; what does not fit meanwhile is dropped and counted.
    pub fn pop_fact(&mut self) -> Option<Fact> {
        self.facts.pop()
    }

    /// How many facts were dropped for want of room, since the model was
    /// made.
    #[must_use]
    pub fn facts_lost(&self) -> u64 {
        self.facts.lost()
    }

    /// The reclaim point: frees what closed in this iteration.
    pub fn reclaim(&mut self) {
        self.channels.reclaim();
        self.attempts.reclaim();
        self.calls.reclaim();
    }
}

/// Handles one event, emitting at most [`max_out`] requests.
pub fn step(model: &mut Model, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    match event {
        Event::Start { reply_to, run, attempt, workstream } => {
            attempt::start(model, env, reply_to, run, attempt, workstream, out);
        }
        Event::Adopt { reply_to, run, attempt } => attempt::adopt(model, env, reply_to, run, attempt, out),
        Event::Cancel { run, attempt } => attempt::cancel(model, run, attempt, out),
        Event::Inbound { run, attempt, event } => call::inbound(model, run, attempt, event, out),
        Event::Relayed { to, answer } => call::relayed(model, to, answer, out),
        Event::Hello { channel, hello } => channel::hello(model, env, channel, hello, out),
        Event::Lost { channel } => channel::lost(model, env, channel),
        Event::Answer { channel, run, attempt, answer, payload } => {
            attempt::answer(model, channel, run, attempt, answer, payload, out);
        }
        Event::Relay { run, attempt, call, body } => call::relay(model, run, attempt, call, body, out),
        Event::Bounced { run, attempt, bounce } => call::bounced(model, run, attempt, bounce, out),
        Event::Told { run, attempt, fact } => call::told(model, run, attempt, fact, out),
    }
}

/// Fires the earliest alarm due at `env.now`, if there is one, emitting at
/// most [`max_out`] requests. A stage fires its alarms after its input
/// events, so a hello that arrived in the same iteration wins over a grace
/// that passed while the loop waited.
pub fn fire(model: &mut Model, env: &Env<Limits>, out: &mut Queue<Request>) {
    if let Some(id) = model.alarms.expire(env.now) {
        attempt::expire(model, id, out);
    }
}

/// Places a waiting attempt, if one can be, emitting at most [`max_out`]
/// requests; with none, placement waits until something changes.
pub fn resume(model: &mut Model, env: &Env<Limits>, out: &mut Queue<Request>) {
    if model.placing {
        attempt::resume(model, env, out);
    }
}
