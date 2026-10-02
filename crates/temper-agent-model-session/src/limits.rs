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
    /// Messages a session's transcript holds, the spec's prompt included.
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
    /// How long a tool call may run. Its deadline goes with it, and a call
    /// that runs out of time is answered as such, to the LLM; the session's
    /// own expiry cancels whatever is still running then.
    pub tool_timeout: Duration,
    /// Facts kept until the parent drains them. Beyond them, facts are
    /// dropped and counted.
    pub facts: u32,
}

/// The most memory the model holds under `limits`, in bytes (6.4), or `None`
/// if it does not fit a `u64` or the limits cannot be honoured.
///
/// It counts the containers, their bookkeeping included, and the payloads, not
/// allocator overhead. The prompts of calls in flight are copies held by the
/// protocol layer, and the texts of yields copies held by the opener, which
/// count them. Facts own nothing beyond their queue.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    if !(1..=MAX_PARALLEL).contains(&limits.parallel_tools) {
        return None;
    }
    let sessions = Slab::<Session>::worst_case(limits.sessions)?;
    let runs = Slab::<Run>::worst_case(runs(limits)?)?;
    let alarms = Deadlines::<Alarm>::worst_case(alarms(limits)?)?;
    let facts = Queue::<Fact>::worst_case(limits.facts)?;
    // Each session owns its transcript's list and up to its byte limit.
    let session = List::<Message>::worst_case(limits.messages)?.checked_add(limits.session_bytes)?;
    let held = u64::from(limits.sessions).checked_mul(session)?;
    sessions.checked_add(runs)?.checked_add(alarms)?.checked_add(facts)?.checked_add(held)
}

/// The run slab's capacity: every session may run a batch, and start the next
/// in the iteration that retired the last, before its slots are reclaimed.
pub(crate) fn runs(limits: &Limits) -> Option<u32> {
    limits.sessions.checked_mul(limits.parallel_tools)?.checked_mul(2)
}

/// The alarm table's capacity: every session may have two alarms armed.
pub(crate) fn alarms(limits: &Limits) -> Option<u32> {
    limits.sessions.checked_mul(2)
}
