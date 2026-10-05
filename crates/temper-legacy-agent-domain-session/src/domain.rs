//! The session child domain's state and its entry points (section 3).

use skein_lib::{Deadlines, Env, Queue, Rng, Slab, Time};
use temper_legacy_agent_domain_tools as tools;

use crate::boundary::{Event, Request};
use crate::facts::{Fact, Facts};
use crate::limits::{self, Limits};
use crate::session::{self, Alarm, Calls, Ready, Session};

/// The most requests an entry point emits per call under `limits`, following
/// the chain to the tools (4.5): the session's own four (a completion's `Used`, priced spend and told turn
/// and what follows it, or an admitted `Open`'s `Opened` and first call);
/// the one batch of runs a step may start, a request each, or the withdraws
/// and cancels closing sends for a batch; and what one step of the tools
/// emits besides (an operation's next one, or the cancels of a kit's close).
/// The parent reserves this much room in `out` before calling it.
#[must_use]
pub const fn max_out(limits: &Limits) -> u32 {
    4_u32.saturating_add(limits.parallel_tools).saturating_add(tools::max_out(&limits.tools))
}

/// The most of those requests that are for the opener (the session's records
/// that name it, and its delegated calls and their withdraws): the session's
/// own four, and a batch of delegated calls, a request each, or the withdraws
/// closing sends for one (a step starts a batch or cancels one, never both).
/// The tools' operations and their cancels never reach the opener.
#[must_use]
pub const fn max_to_opener(limits: &Limits) -> u32 {
    4_u32.saturating_add(limits.parallel_tools)
}

/// The session child domain's state.
#[derive(Debug)]
pub struct Domain {
    pub(crate) sessions: Slab<Session>,
    /// What runs the sessions' tool calls: the tools, which the session owns.
    pub(crate) calls: Calls,
    pub(crate) alarms: Deadlines<Alarm>,
    /// Sessions resting after a batch answered at once.
    pub(crate) ready: Ready,
    pub(crate) rng: Rng,
    pub(crate) facts: Facts,
}

impl Domain {
    /// A domain with room for `limits`, drawing randomness from `seed`.
    #[must_use]
    pub fn new(limits: &Limits, seed: u64) -> Domain {
        let alarms = limits::alarms(limits).expect("worst_case accepted the limits");
        let runs = limits::runs(limits).expect("worst_case accepted the limits");
        let calls = Calls {
            runs: Slab::with_capacity(runs),
            tools: tools::Domain::new(&limits.tools),
            out: Queue::with_capacity(tools::max_out(&limits.tools)),
        };
        Domain {
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
    /// root domain, which calls [`fire`].
    #[must_use]
    pub fn is_due(&self, now: Time) -> bool {
        match self.alarms.next() {
            Some(at) => at <= now,
            None => false,
        }
    }

    /// Whether a session is ready to go on. While one is, the loop resumes the
    /// root domain, which calls [`resume`], at the start of the domain's
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
    /// since the domain was made.
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
pub fn step(domain: &mut Domain, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    match event {
        Event::Open { opener, spec } => session::open(domain, env, opener, spec, out),
        Event::OpenV2 { opener, spec } => session::open_v2(domain, env, opener, spec, out),
        Event::AnsweredV2 { owner, text, error, spent } => {
            session::delegate_ended_v2(domain, env, owner, crate::llm::Returned::Text { text, error }, spent, out);
        }
        Event::AnswerCancelledV2 { owner, spent } => {
            session::delegate_ended_v2(domain, env, owner, crate::llm::Returned::Withdrawn, spent, out);
        }
        Event::Continue { session, content } => session::continued(domain, env, session, content, out),
        Event::Close { session } => session::close(domain, env, session, out),
        Event::Completed { owner, completion } => session::completed(domain, env, owner, completion, out),
        Event::Failed { owner, failure } => session::failed(domain, env, owner, failure, out),
        Event::Cancelled { owner } => session::cancelled(domain, env, owner, out),
        Event::Done { owner, done } => session::io_done(domain, env, owner, done, out),
        Event::Answered { owner, answer } => session::delegate_answered(domain, env, owner, answer, out),
        Event::AnswerCancelled { owner } => session::delegate_cancelled(domain, env, owner, out),
    }
    session::pass_on_facts(domain, env);
}

/// Fires the earliest alarm due at `env.now`, if there is one, emitting at most
/// [`max_out`] requests. A stage fires its alarms after its input events, so
/// progress that arrived in the same iteration wins over a deadline that passed
/// while the loop waited.
pub fn fire(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    let Some(alarm) = domain.alarms.expire(env.now) else {
        return;
    };
    match alarm {
        Alarm::Expiry { session } => session::expire(domain, env, session, out),
        Alarm::Retry { session } => session::retry(domain, env, session, out),
    }
    session::pass_on_facts(domain, env);
}

/// Starts a session on the ready list again, if one is, emitting at most
/// [`max_out`] requests: one that rested in an earlier iteration, after a
/// batch the tools answered within the step that started it.
pub fn resume(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    let Some(id) = domain.ready.pop() else {
        return;
    };
    session::rested(domain, env, id, out);
    session::pass_on_facts(domain, env);
}
