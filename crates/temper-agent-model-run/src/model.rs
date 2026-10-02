//! The run sub-model's state and its entry points (section 3).

use temper_lib::{Deadlines, Env, Queue, Slab, Time};

use crate::boundary::{Event, Request};
use crate::call::Calls;
use crate::facts::{self, Fact, Facts};
use crate::limits::Limits;
use crate::run::{self, Alarm, Conversation, Run};

/// The most requests an entry point emits per call: an admitted start names
/// the run and opens its main conversation or asks io for its first look; a
/// check goes with its notice to the worker; a call returns as main is
/// closed. The parent reserves this much room in `out` before calling it.
pub const MAX_OUT: u32 = 2;

/// The run sub-model's state.
#[derive(Debug)]
pub struct Model {
    pub(crate) runs: Slab<Run>,
    pub(crate) conversations: Slab<Conversation>,
    /// Calls of conversations to the run.
    pub(crate) calls: Calls,
    pub(crate) alarms: Deadlines<Alarm>,
    pub(crate) facts: Facts,
}

impl Model {
    /// A model with room for `limits`.
    #[must_use]
    pub fn new(limits: &Limits) -> Model {
        Model {
            runs: Slab::with_capacity(limits.runs),
            conversations: Slab::with_capacity(limits.conversations),
            calls: Calls::with_capacity(limits.calls),
            alarms: Deadlines::with_capacity(limits.runs.saturating_add(limits.calls)),
            facts: Facts::with_capacity(limits.facts),
        }
    }

    /// Runs present, closed ones included until they are reclaimed.
    #[must_use]
    pub fn runs(&self) -> u32 {
        self.runs.len()
    }

    /// Conversations present, ended ones included until they are reclaimed.
    #[must_use]
    pub fn conversations(&self) -> u32 {
        self.conversations.len()
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

    /// Calls of conversations to the run, returned ones included until they
    /// are reclaimed.
    #[must_use]
    pub fn calls(&self) -> u32 {
        self.calls.len()
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
        self.runs.reclaim();
        self.conversations.reclaim();
        self.calls.reclaim();
    }
}

/// Handles one event, emitting at most [`MAX_OUT`] requests.
pub fn step(model: &mut Model, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    model.facts.begin();
    let mark = out.len();
    take(model, env, event, out);
    facts::tell(&mut model.facts, &model.runs, &model.conversations, out, mark);
}

fn take(model: &mut Model, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    match event {
        Event::Start { reply_to, worker, charter } => run::start(model, env, reply_to, worker, charter, out),
        Event::Cancel { run } => run::cancel(model, run, out),
        Event::Started { conversation, peer } => run::started(model, conversation, peer, out),
        Event::Yielded { conversation, stop, text } => run::yielded(model, env, conversation, stop, &text, out),
        Event::Used { conversation, spend } => run::used(model, conversation, spend, out),
        Event::Ended { conversation, end, spend } => run::ended(model, conversation, end, spend, out),
        Event::Read { owner, read } => run::read(model, env, owner, read, out),
        Event::Probed { owner, executable } => run::probed(model, env, owner, executable, out),
        Event::Delegated { conversation, call, ask, deadline } => {
            run::delegated(model, env, conversation, call, ask, deadline, out);
        }
        Event::Withdraw { conversation, call } => run::withdraw(model, conversation, call, out),
        Event::Checked { owner, ran } => run::checked(model, env, owner, ran, out),
        Event::Aborted { owner } => run::aborted(model, owner, out),
        Event::Pushed { owner, push } => run::pushed(model, owner, push, out),
        Event::HostCancelled { owner } => run::host_cancelled(model, owner, out),
    }
}

/// Fires the earliest alarm due at `env.now`, if there is one, emitting at most
/// [`MAX_OUT`] requests. A stage fires its alarms after its input events, so
/// progress that arrived in the same iteration wins over a deadline that passed
/// while the loop waited.
pub fn fire(model: &mut Model, env: &Env<Limits>, out: &mut Queue<Request>) {
    let Some(alarm) = model.alarms.expire(env.now) else {
        return;
    };
    model.facts.begin();
    let mark = out.len();
    match alarm {
        Alarm::Deadline { run } => run::deadline(model, run, out),
        Alarm::Call { call } => run::expired(model, call, out),
    }
    facts::tell(&mut model.facts, &model.runs, &model.conversations, out, mark);
}
