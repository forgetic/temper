//! The session sub-model's state and its entry points (section 3).

use temper_agent_model_tools as tools;
use temper_lib::{Deadlines, Env, Queue, Rng, Slab, Time};

use crate::boundary::{Event, Request};
use crate::facts::{Fact, Facts};
use crate::limits::{self, Limits};
use crate::session::{self, Alarm, Calls, Ready, Session};

/// The most requests an entry point emits per call under `limits`, following
/// the chain to the tools (4.5): the session's own two (a completion's `Used`
/// and what follows it, or an admitted `Open`'s `Opened` and first call);
/// the one batch of runs a step may start, a request each, or the withdraws
/// and cancels closing sends for a batch; and what one step of the tools
/// emits besides (an operation's next one, or the cancels of a kit's close).
/// The parent reserves this much room in `out` before calling it.
#[must_use]
pub const fn max_out(limits: &Limits) -> u32 {
    2_u32.saturating_add(limits.parallel_tools).saturating_add(tools::max_out(&limits.tools))
}

/// The session sub-model's state.
#[derive(Debug)]
pub struct Model {
    pub(crate) sessions: Slab<Session>,
    /// What runs the sessions' tool calls: the tools, which the session owns.
    pub(crate) calls: Calls,
    pub(crate) alarms: Deadlines<Alarm>,
    /// Sessions resting after a batch answered at once.
    pub(crate) ready: Ready,
    pub(crate) rng: Rng,
    pub(crate) facts: Facts,
}

impl Model {
    /// A model with room for `limits`, drawing randomness from `seed`.
    #[must_use]
    pub fn new(limits: &Limits, seed: u64) -> Model {
        let alarms = limits::alarms(limits).expect("worst_case accepted the limits");
        let runs = limits::runs(limits).expect("worst_case accepted the limits");
        let calls = Calls {
            runs: Slab::with_capacity(runs),
            tools: tools::Model::new(&limits.tools),
            out: Queue::with_capacity(tools::max_out(&limits.tools)),
        };
        Model {
            sessions: Slab::with_capacity(limits.sessions),
            calls,
            alarms: Deadlines::with_capacity(alarms),
            ready: Ready::with_capacity(limits.sessions),
            rng: Rng::new(seed),
            facts: Facts::with_capacity(limits.facts),
        }
    }

    /// Sessions present, closed ones included until they are reclaimed.
    #[must_use]
    pub fn sessions(&self) -> u32 {
        self.sessions.len()
    }

    /// Tool runs present, the tools' and the opener's, ended ones included
    /// until they are reclaimed.
    #[must_use]
    pub fn runs(&self) -> u32 {
        self.calls.runs.len()
    }

    /// Kits present, closed ones included until they are reclaimed.
    #[must_use]
    pub fn kits(&self) -> u32 {
        self.calls.tools.kits()
    }

    /// Calls the tools are running, answered ones included until they are
    /// reclaimed.
    #[must_use]
    pub fn jobs(&self) -> u32 {
        self.calls.tools.jobs()
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

    /// Whether a session is ready to go on. While one is, the loop resumes the
    /// top-level model, which calls [`resume`], at the start of the model's
    /// stage, before its input events.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.ready.is_ready()
    }

    /// The oldest fact not yet drained, the tools' among them. The parent
    /// drains them at its own pace; what does not fit meanwhile is dropped and
    /// counted.
    pub fn pop_fact(&mut self) -> Option<Fact> {
        self.facts.pop()
    }

    /// How many facts were dropped for want of room, the tools' included,
    /// since the model was made.
    #[must_use]
    pub fn facts_lost(&self) -> u64 {
        self.facts.lost().saturating_add(self.calls.tools.facts_lost())
    }

    /// The reclaim point: frees what closed in this iteration.
    pub fn reclaim(&mut self) {
        self.sessions.reclaim();
        self.calls.runs.reclaim();
        self.calls.tools.reclaim();
        self.ready.promote();
    }
}

/// Handles one event, emitting at most [`max_out`] requests.
pub fn step(model: &mut Model, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    match event {
        Event::Open { opener, spec } => session::open(model, env, opener, spec, out),
        Event::Continue { session, content } => session::continued(model, env, session, content, out),
        Event::Close { session } => session::close(model, env, session, out),
        Event::Completed { owner, completion } => session::completed(model, env, owner, completion, out),
        Event::Failed { owner, failure } => session::failed(model, env, owner, failure, out),
        Event::Cancelled { owner } => session::cancelled(model, env, owner, out),
        Event::Done { owner, done } => session::io_done(model, env, owner, done, out),
        Event::Answered { owner, answer } => session::delegate_answered(model, env, owner, answer, out),
        Event::AnswerCancelled { owner } => session::delegate_cancelled(model, env, owner, out),
    }
    session::pass_on_facts(model, env);
}

/// Fires the earliest alarm due at `env.now`, if there is one, emitting at most
/// [`max_out`] requests. A stage fires its alarms after its input events, so
/// progress that arrived in the same iteration wins over a deadline that passed
/// while the loop waited.
pub fn fire(model: &mut Model, env: &Env<Limits>, out: &mut Queue<Request>) {
    let Some(alarm) = model.alarms.expire(env.now) else {
        return;
    };
    match alarm {
        Alarm::Expiry { session } => session::expire(model, env, session, out),
        Alarm::Retry { session } => session::retry(model, env, session, out),
    }
    session::pass_on_facts(model, env);
}

/// Starts a session on the ready list again, if one is, emitting at most
/// [`max_out`] requests: one that rested in an earlier iteration, after a
/// batch the tools answered within the step that started it.
pub fn resume(model: &mut Model, env: &Env<Limits>, out: &mut Queue<Request>) {
    let Some(id) = model.ready.pop() else {
        return;
    };
    session::rested(model, env, id, out);
    session::pass_on_facts(model, env);
}
