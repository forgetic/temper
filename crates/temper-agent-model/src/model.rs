//! The model layer's state and its entry points (section 3).

use temper_lib::{Deadlines, Env, Queue, Rng, Slab, Time};

use crate::boundary::{Event, Request};
use crate::limits::Limits;
use crate::session::{self, Alarm, Session};

/// The most requests an entry point emits per call. The loop reserves this
/// much room in `out` before calling it.
pub const MAX_OUT: u32 = 1;

/// The agent model's state.
#[derive(Debug)]
pub struct Model {
    pub(crate) sessions: Slab<Session>,
    pub(crate) alarms: Deadlines<Alarm>,
    pub(crate) rng: Rng,
}

impl Model {
    /// A model with room for `limits`, drawing randomness from `seed`.
    #[must_use]
    pub fn new(limits: &Limits, seed: u64) -> Model {
        let alarms = limits.sessions.checked_mul(2).expect("worst_case accepted the limits");
        Model {
            sessions: Slab::with_capacity(limits.sessions),
            alarms: Deadlines::with_capacity(alarms),
            rng: Rng::new(seed),
        }
    }

    /// Sessions present, closed ones included until they are reclaimed.
    #[must_use]
    pub fn sessions(&self) -> u32 {
        self.sessions.len()
    }

    /// When the earliest alarm falls due.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        self.alarms.next()
    }

    /// Whether an alarm is due at `now`. The loop calls [`fire`] while one is.
    #[must_use]
    pub fn is_due(&self, now: Time) -> bool {
        match self.alarms.next() {
            Some(at) => at <= now,
            None => false,
        }
    }

    /// The reclaim point: frees what closed in this iteration.
    pub fn reclaim(&mut self) {
        self.sessions.reclaim();
    }
}

/// Handles one event, emitting at most [`MAX_OUT`] requests.
pub fn step(model: &mut Model, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    match event {
        Event::Run { reply_to, task } => session::run(model, env, reply_to, task, out),
        Event::Completed { owner, completion } => session::completed(model, env, owner, completion, out),
        Event::Failed { owner, failure } => session::failed(model, env, owner, failure, out),
        Event::Cancelled { owner } => session::cancelled(model, owner, out),
        Event::ToolDone { owner, output, error } => session::tool_done(model, env, owner, output, error, out),
        Event::ToolCancelled { owner } => session::tool_cancelled(model, owner, out),
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
        Alarm::Expiry { session } => session::expire(model, session, out),
        Alarm::Retry { session } => session::retry(model, env, session, out),
    }
}
