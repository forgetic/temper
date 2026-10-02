use temper_agent_model_tools as tools;
use temper_lib::{Deadlines, Duration, List, Queue, Slab};

use crate::boundary::Budget;
use crate::facts::Fact;
use crate::llm::Message;
use crate::session::{Alarm, Run, Session};

/// The most tool calls a session runs at once: what `Limits::parallel_tools`
/// may be, and what bounds the requests a step emits.
pub const MAX_PARALLEL: u32 = 8;

/// The session sub-model's limits (section 7), handed by its parent to every
/// step read-only.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    /// Sessions at once. An `Open` beyond them is refused as busy.
    pub sessions: u32,
    /// Messages a session's transcript holds, the spec's prompt included: at
    /// least two, the prompt and an answer.
    pub messages: u32,
    /// Bytes a session holds: its spec, its transcript and the tool results
    /// it is collecting, each block counted at its fixed size plus its payload.
    pub session_bytes: u64,
    /// The largest budget a spec may ask for, dimension by dimension. Its time
    /// is the longest a session may live.
    pub budget: Budget,
    /// Owned tool calls a session runs at once: adjacent calls that read run
    /// together, up to this many, and a call that writes runs alone. Between
    /// one and [`MAX_PARALLEL`].
    pub parallel_tools: u32,
    /// The largest `max_tokens` a spec may ask for.
    pub max_tokens: u32,
    /// Retries of a call that failed transiently, after which the session
    /// fails.
    pub retries: u32,
    /// The wait before the first retry, doubled for each one after it, with
    /// jitter.
    pub backoff_base: Duration,
    /// The longest wait before a retry, unless the provider asks for longer.
    pub backoff_max: Duration,
    /// How long the protocol layer gives each call.
    pub call_timeout: Duration,
    /// How long a tool call may run, and no later than the session's time
    /// runs out. Its deadline goes with it, and a call that runs out of time
    /// is answered as such, to the LLM.
    pub tool_timeout: Duration,
    /// The same for a delegated call, which the opener serves.
    pub delegate_timeout: Duration,
    /// Facts kept until the parent drains them, the tools' passed on among
    /// them. Beyond them, facts are dropped and counted.
    pub facts: u32,
    /// The tools sub-model's, which the session owns: a kit for each session,
    /// with room for its widest batch.
    pub tools: tools::Limits,
}

/// The most memory the model holds under `limits`, in bytes (6.4), or `None`
/// if it does not fit a `u64` or the limits cannot be honoured: a parallel
/// batch wider than [`MAX_PARALLEL`] or than the tools run for a kit at once
/// (so that a read never meets `Busy`), fewer kits than sessions, or a
/// transcript too short for a prompt and its answer.
///
/// It is the session's own, the tools' it owns, and the queue that holds what
/// the tools emit in a step. It counts the containers, their bookkeeping
/// included, and the payloads, not allocator overhead. The prompts of calls in
/// flight are copies held by the protocol layer, and the texts of yields copies
/// held by the opener, which count them. Facts own nothing beyond their queue.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    let parallel = limits.parallel_tools;
    if !(1..=MAX_PARALLEL).contains(&parallel) || parallel > limits.tools.calls || limits.messages < 2 {
        return None;
    }
    if limits.tools.kits < limits.sessions {
        return None;
    }
    let sessions = Slab::<Session>::worst_case(limits.sessions)?;
    let runs = Slab::<Run>::worst_case(runs(limits)?)?;
    let alarms = Deadlines::<Alarm>::worst_case(alarms(limits)?)?;
    let facts = Queue::<Fact>::worst_case(limits.facts)?;
    let tools = tools::worst_case(&limits.tools)?;
    let tools_out = Queue::<tools::Request>::worst_case(tools::max_out(&limits.tools))?;
    // Each session owns its transcript's list and up to its byte limit.
    let session = List::<Message>::worst_case(limits.messages)?.checked_add(limits.session_bytes)?;
    let held = u64::from(limits.sessions).checked_mul(session)?;
    sessions
        .checked_add(runs)?
        .checked_add(alarms)?
        .checked_add(facts)?
        .checked_add(tools)?
        .checked_add(tools_out)?
        .checked_add(held)
}

/// The run slab's capacity: two batches a session. A session starts at most
/// one batch in a step, and a batch the tools answered in the step that
/// started it goes on at the next instant, after the reclaim point (see the
/// session module). So in one iteration a session holds the runs of at most
/// two batches: one ending, and the next it starts.
pub(crate) fn runs(limits: &Limits) -> Option<u32> {
    limits.sessions.checked_mul(limits.parallel_tools)?.checked_mul(2)
}

/// The alarm table's capacity: every session may have three alarms armed.
pub(crate) fn alarms(limits: &Limits) -> Option<u32> {
    limits.sessions.checked_mul(3)
}
