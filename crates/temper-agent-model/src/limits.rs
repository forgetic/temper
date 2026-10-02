use temper_agent_model_run::{self as run, Ask};
use temper_agent_model_session as session;
use temper_lib::{Id, Map, Queue, Set, Slab, Token};

use crate::facts::Fact;
use crate::model::{Flight, Handoff};
use crate::peer::{self, Peer};

/// The agent model's limits (section 7), handed to every step read-only: its
/// sub-models', each handed down to the one it bounds. The session's include
/// its tools'.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    pub run: run::Limits,
    pub session: session::Limits,
}

/// The most memory the model holds under `limits`, in bytes (6.4), or `None`
/// if it does not fit a `u64` or the limits cannot be honoured: the
/// sub-models' own, or limits under which a session would refuse what the run
/// asks of it within its own limits (fewer sessions than the run's
/// conversations, a smaller budget or answer, fewer repositories).
///
/// It is the sub-models', plus what the top level keeps: a peer for each
/// conversation, with the asks and answers of its session's tickets, the maps
/// that find peers and the delegated calls in flight, the ready list, the
/// queues that hold what each sub-model emits in a step until it is routed,
/// and the facts. A peer's asks hold at most its session's byte limit; its
/// answers are charged to the session, and counted with it, but for those of
/// the batch it waits for ([`uncharged`]). What the queued requests own is
/// counted where they end up.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    let Limits { run: run_limits, session: session_limits } = limits;
    let budget = run_limits.budget;
    let ceiling = session_limits.budget;
    let fits = budget.turns <= ceiling.turns
        && budget.input <= ceiling.input
        && budget.output <= ceiling.output
        && budget.cache_read <= ceiling.cache_read
        && budget.cache_write <= ceiling.cache_write
        && budget.time <= ceiling.time;
    if !fits
        || run_limits.conversations > session_limits.sessions
        || run_limits.max_tokens > session_limits.max_tokens
        || run_limits.repositories > session_limits.tools.repos
    {
        return None;
    }
    let children = run::worst_case(run_limits)?.checked_add(session::worst_case(session_limits)?)?;
    let peers = run_limits.conversations;
    let tickets = Map::<u64, Ask>::worst_case(peer::asks(session_limits))?
        .checked_add(session_limits.session_bytes)?
        .checked_add(Map::<u64, run::Returned>::worst_case(peer::answers(session_limits))?)?
        .checked_add(uncharged(limits)?)?;
    let held = Slab::<Peer>::worst_case(peers)?.checked_add(u64::from(peers).checked_mul(tickets)?)?;
    let found = Map::<Token, Id<Peer>>::worst_case(peers)?.checked_mul(2)?;
    let flights = Map::<Token, Flight>::worst_case(flights(limits)?)?;
    let ready = Set::<Handoff>::worst_case(handoffs(limits)?)?.checked_mul(2)?;
    let run_out = Queue::<run::Request>::worst_case(run_out(limits))?;
    let session_out = Queue::<session::Request>::worst_case(session_out(limits))?;
    let facts = Queue::<Fact>::worst_case(facts(limits)?)?;
    children
        .checked_add(held)?
        .checked_add(found)?
        .checked_add(flights)?
        .checked_add(ready)?
        .checked_add(run_out)?
        .checked_add(session_out)?
        .checked_add(facts)
}

/// What a peer holds of the run's answers that its session has not charged
/// for: the payloads of the batch it waits for, each answer's from the run's
/// `Return` until it reaches the session from the ready list, and for good
/// once it reaches a session that is closing, or that has no room for it.
/// The fixed size of their tickets is among a peer's answers.
pub(crate) fn uncharged(limits: &Limits) -> Option<u64> {
    u64::from(limits.session.parallel_tools).checked_mul(peer::payload(&limits.run)?)
}

/// Delegated calls in flight at once: a batch of each session.
pub(crate) fn flights(limits: &Limits) -> Option<u32> {
    limits.run.conversations.checked_mul(limits.session.parallel_tools)
}

/// Hand-offs on the ready list at once: a close of each conversation, and the
/// answer to each delegated call.
pub(crate) fn handoffs(limits: &Limits) -> Option<u32> {
    limits.run.conversations.checked_add(flights(limits)?)
}

/// Facts kept until the loop drains them: as many as both sub-models keep.
pub(crate) fn facts(limits: &Limits) -> Option<u32> {
    limits.run.facts.checked_add(limits.session.facts)
}

/// Room for what the run emits in an entry point: its most, for each step it
/// takes (see the model module).
pub(crate) const fn run_out(limits: &Limits) -> u32 {
    run_steps(limits).saturating_mul(run::MAX_OUT)
}

/// Room for what the session sub-model emits in an entry point.
pub(crate) const fn session_out(limits: &Limits) -> u32 {
    session_steps(limits).saturating_mul(session::max_out(&limits.session))
}

/// The most steps an entry point takes of the session sub-model: its own, if
/// it is for the sessions, and one for each hand-off the run makes at once
/// to a session in answer to what that step emitted.
pub(crate) const fn session_steps(limits: &Limits) -> u32 {
    let session = session::max_out(&limits.session);
    1_u32.saturating_add(session.saturating_mul(run::MAX_OUT))
}

/// The most steps an entry point takes of the run: one for each request the
/// session's first step emits, one for each hand-off the run makes at once in
/// answer, which may be refused in the run's terms without a session, and one
/// for each request the sessions it hands off to emit.
pub(crate) const fn run_steps(limits: &Limits) -> u32 {
    let session = session::max_out(&limits.session);
    let at_once = session.saturating_mul(run::MAX_OUT);
    session.saturating_add(at_once).saturating_add(at_once.saturating_mul(session))
}
