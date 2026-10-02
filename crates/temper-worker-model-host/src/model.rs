//! The host sub-model's state and its entry points (section 3).

use temper_lib::{Env, Id, Map, Queue, Slab, Token};

use crate::boundary::{Event, Reason, Request};
use crate::call::Call;
use crate::facts::{Fact, Facts};
use crate::hosted::{self, Hosted};
use crate::limits::{self, Limits};

/// The most requests an entry point emits per call under `limits`: a run
/// that leaves live answers each of its relayed calls in flight, then stops
/// its agent, or, its agent gone, saves or releases its workspace and answers;
/// and an agent that starts is handed every inbound event held for it. The parent
/// reserves this much room in `out` before calling it.
#[must_use]
pub const fn max_out(limits: &Limits) -> u32 {
    let most = if limits.held > limits.run_calls { limits.held } else { limits.run_calls };
    most.saturating_add(2)
}

/// The host sub-model's state.
#[derive(Debug)]
pub struct Model {
    /// The run slots.
    pub(crate) hosted: Slab<Hosted>,
    /// The runs not yet answered, by the engine's names for them.
    pub(crate) names: Map<Token, Id<Hosted>>,
    /// Host calls in flight, across runs.
    pub(crate) calls: Slab<Call>,
    /// Runs to cancel, each for its reason, one per resume.
    pub(crate) ready: Map<Id<Hosted>, Reason>,
    pub(crate) facts: Facts,
    /// The worker is shutting down: it admits no more runs.
    pub(crate) shut: bool,
}

impl Model {
    /// A model with room for `limits`.
    #[must_use]
    pub fn new(limits: &Limits) -> Model {
        let calls = limits::calls(limits).expect("worst_case accepted the limits");
        Model {
            hosted: Slab::with_capacity(limits.slots),
            names: Map::with_capacity(limits.slots),
            calls: Slab::with_capacity(calls),
            ready: Map::with_capacity(limits.slots),
            facts: Facts::with_capacity(limits.facts),
            shut: false,
        }
    }

    /// Runs hosted: the slots taken, closed runs included until they are
    /// reclaimed.
    #[must_use]
    pub fn hosted(&self) -> u32 {
        self.hosted.len()
    }

    /// Host calls in flight, closed ones included until they are reclaimed.
    #[must_use]
    pub fn calls(&self) -> u32 {
        self.calls.len()
    }

    /// Whether a run is ready to be cancelled. While one is, the loop resumes
    /// the top-level model, which calls [`resume`], at the start of the
    /// model's stage, before its input events.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        !self.ready.is_empty()
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
        self.hosted.reclaim();
        self.calls.reclaim();
    }
}

/// Handles one event, emitting at most [`max_out`] requests.
pub fn step(model: &mut Model, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    match event {
        Event::Assign { reply_to, assignment } => hosted::assign(model, env, reply_to, assignment, out),
        Event::Inbound { run, attempt, event } => hosted::inbound(model, env, run, attempt, event, out),
        Event::Cancel { run, attempt } => hosted::cancel(model, env, run, attempt, out),
        Event::Relayed { run, attempt, call, answer } => hosted::relayed(model, run, attempt, call, answer, out),
        Event::CancelAll { reason } => hosted::cancel_all(model, reason),
        Event::Report => hosted::report(model, out),
        Event::Prepared { owner, workspace } => hosted::prepared(model, owner, workspace, out),
        Event::Unprepared { owner, failure, detail } => hosted::unprepared(model, env, owner, failure, detail, out),
        Event::Started { owner, agent } => hosted::started(model, env, owner, agent, out),
        Event::Called { owner, call, ask } => hosted::called(model, env, owner, call, ask, out),
        Event::Yielded { owner } => hosted::yielded(model, owner, out),
        Event::Finished { owner, finish } => hosted::finished(model, env, owner, finish, out),
        Event::Faulted { owner, fault } => hosted::faulted(model, env, owner, fault, out),
        Event::Gone { owner, detail } => hosted::gone(model, env, owner, detail, out),
        Event::Pushed { owner, push } => hosted::pushed(model, owner, push, out),
        Event::Saved { owner, save } => hosted::saved(model, owner, save, out),
    }
}

/// Cancels a run on the ready list, if one is, emitting at most [`max_out`]
/// requests: one of those a `CancelAll` named, for its reason.
pub fn resume(model: &mut Model, env: &Env<Limits>, out: &mut Queue<Request>) {
    hosted::resume(model, env, out);
}
