//! The run sub-model's state and its entry points (section 3).

use temper_lib::{Deadlines, Env, Queue, Slab, Time};

use crate::boundary::{Event, Request};
use crate::limits::Limits;
use crate::run::{self, Alarm, Conversation, Run};

/// The most requests an entry point emits per call: an admitted start names
/// the run and opens its main conversation. The parent reserves this much room
/// in `out` before calling it.
pub const MAX_OUT: u32 = 2;

/// The run sub-model's state.
#[derive(Debug)]
pub struct Model {
    pub(crate) runs: Slab<Run>,
    pub(crate) conversations: Slab<Conversation>,
    pub(crate) alarms: Deadlines<Alarm>,
}

impl Model {
    /// A model with room for `limits`.
    #[must_use]
    pub fn new(limits: &Limits) -> Model {
        Model {
            runs: Slab::with_capacity(limits.runs),
            conversations: Slab::with_capacity(limits.conversations),
            alarms: Deadlines::with_capacity(limits.runs),
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

    /// The reclaim point: frees what closed in this iteration.
    pub fn reclaim(&mut self) {
        self.runs.reclaim();
        self.conversations.reclaim();
    }
}

/// Handles one event, emitting at most [`MAX_OUT`] requests.
pub fn step(model: &mut Model, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    match event {
        Event::Start { reply_to, worker, charter } => run::start(model, env, reply_to, worker, charter, out),
        Event::Cancel { run } => run::cancel(model, run, out),
        Event::Started { conversation, peer } => run::started(model, conversation, peer, out),
        Event::Yielded { conversation, stop, text: _ } => run::yielded(model, conversation, stop, out),
        Event::Used { conversation, spend } => run::used(model, conversation, spend, out),
        Event::Ended { conversation, end, spend } => run::ended(model, conversation, end, spend, out),
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
    match alarm {
        Alarm::Deadline { run } => run::deadline(model, run, out),
    }
}
