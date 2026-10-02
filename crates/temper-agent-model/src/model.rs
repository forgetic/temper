//! The model's state and its entry points (section 3). Each hands what it is
//! given to the sub-model it is for, and routes what that sub-model emits back
//! out before it returns.

use temper_agent_model_session as session;
use temper_lib::{Env, Queue, Time};

use crate::boundary::{Event, Request};
use crate::limits::Limits;
use crate::route;

/// The most requests an entry point emits per call: the session's, since each
/// of its requests routes to one of ours. The loop reserves this much room in
/// `out` before calling it.
pub const MAX_OUT: u32 = session::MAX_OUT;

/// The agent model's state: its sub-models', and room for what they emit
/// within a step.
#[derive(Debug)]
pub struct Model {
    session: session::Model,
    /// What the session emits in one step, until it is routed out. Empty
    /// between steps.
    session_out: Queue<session::Request>,
}

impl Model {
    /// A model with room for `limits`, drawing randomness from `seed`.
    #[must_use]
    pub fn new(limits: &Limits, seed: u64) -> Model {
        Model {
            session: session::Model::new(&limits.session, seed),
            session_out: Queue::with_capacity(session::MAX_OUT),
        }
    }

    /// Sessions present, closed ones included until they are reclaimed.
    #[must_use]
    pub fn sessions(&self) -> u32 {
        self.session.sessions()
    }

    /// When the earliest alarm falls due.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        self.session.next_deadline()
    }

    /// Whether an alarm is due at `now`. The loop calls [`fire`] while one is.
    #[must_use]
    pub fn is_due(&self, now: Time) -> bool {
        self.session.is_due(now)
    }

    /// The oldest fact the sessions told and the loop has not drained yet.
    /// They are the session's own for now, passed through as they are.
    pub fn pop_fact(&mut self) -> Option<session::Fact> {
        self.session.pop_fact()
    }

    /// How many facts were dropped for want of room.
    #[must_use]
    pub fn facts_lost(&self) -> u64 {
        self.session.facts_lost()
    }

    /// The reclaim point: frees what closed in this iteration.
    pub fn reclaim(&mut self) {
        self.session.reclaim();
    }
}

/// Handles one event, emitting at most [`MAX_OUT`] requests.
pub fn step(model: &mut Model, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    session::step(&mut model.session, &session_env(env), route::event(event), &mut model.session_out);
    route_out(model, out);
}

/// Fires the earliest alarm due at `env.now`, if there is one, emitting at most
/// [`MAX_OUT`] requests. A stage fires its alarms after its input events, so
/// progress that arrived in the same iteration wins over a deadline that passed
/// while the loop waited.
pub fn fire(model: &mut Model, env: &Env<Limits>, out: &mut Queue<Request>) {
    session::fire(&mut model.session, &session_env(env), &mut model.session_out);
    route_out(model, out);
}

/// What the session reads: this iteration's time, and its own limits.
fn session_env(env: &Env<Limits>) -> Env<session::Limits> {
    Env { now: env.now, limits: env.limits.session }
}

/// Routes what the session emitted into `out`, leaving the session's queue
/// empty for the next step.
fn route_out(model: &mut Model, out: &mut Queue<Request>) {
    for _ in 0..session::MAX_OUT {
        let Some(request) = model.session_out.pop() else { break };
        out.push(route::request(request));
    }
    assert!(model.session_out.is_empty(), "the session emits at most its MAX_OUT");
}
