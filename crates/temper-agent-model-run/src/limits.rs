use temper_lib::{Deadlines, Slab};

use crate::budget::Budget;
use crate::run::{Alarm, Conversation, Run};

/// The run sub-model's limits (section 7), handed by its parent to every step
/// read-only.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    /// Runs at once. A start beyond them is refused as busy.
    pub runs: u32,
    /// Conversations at once, across runs. A start with no room for its main
    /// conversation is refused as busy.
    pub conversations: u32,
    /// Bytes a run holds: its charter, each part held in a box counted at its
    /// fixed size plus its payload.
    pub run_bytes: u64,
    /// Repositories a checkout may list.
    pub repositories: u32,
    /// Outlets a charter may grant.
    pub outlets: u32,
    /// Verdicts an outcome spec may list.
    pub verdicts: u32,
    /// The largest budget a charter may ask for, part by part.
    pub budget: Budget,
    /// The largest `max_tokens` a charter's LLM may ask for.
    pub max_tokens: u32,
}

/// The most memory the model holds under `limits`, in bytes (6.4), or `None`
/// if it does not fit a `u64`.
///
/// It counts the containers, their bookkeeping included, and the payloads, not
/// allocator overhead. What a run sends a conversation is a copy, which the
/// conversation counts.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    let runs = Slab::<Run>::worst_case(limits.runs)?;
    let conversations = Slab::<Conversation>::worst_case(limits.conversations)?;
    let alarms = Deadlines::<Alarm>::worst_case(limits.runs)?;
    // Each run holds its charter, up to its byte limit.
    let charters = u64::from(limits.runs).checked_mul(limits.run_bytes)?;
    runs.checked_add(conversations)?.checked_add(alarms)?.checked_add(charters)
}
