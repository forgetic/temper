use temper_lib::{Deadlines, Duration, List, Slab};

use crate::budget::Budget;
use crate::call::Calls;
use crate::prepare::Guide;
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
    /// Calls of conversations to the run in flight at once, across runs. A
    /// call beyond them is answered as busy.
    pub calls: u32,
    /// The largest budget a charter may ask for, part by part.
    pub budget: Budget,
    /// The largest `max_tokens` a charter's LLM may ask for.
    pub max_tokens: u32,
    /// Nudges a run gives its LLM when it stops without finishing, after which
    /// the run fails.
    pub nudges: u32,
    /// The most bytes of a repository's `AGENTS.md` a run reads and puts in
    /// its system text.
    pub guide_bytes: u32,
    /// How long io has for each look in the checkout.
    pub io_timeout: Duration,
    /// The most bytes an outcome declared to `finish` may hold, as a run
    /// counts them. A larger one is rejected.
    pub outcome_bytes: u64,
    /// How long a repository's checks may run.
    pub check_timeout: Duration,
    /// The most bytes of a failed check's output the LLM is shown: its tail.
    pub check_tail: u32,
}

/// The most memory the model holds under `limits`, in bytes (6.4), or `None`
/// if it does not fit a `u64`.
///
/// It counts the containers, their bookkeeping included, and the payloads, not
/// allocator overhead. What a run sends is a copy, which its receiver counts;
/// what it receives and only passes on (a check's output) is the sender's.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    let runs = Slab::<Run>::worst_case(limits.runs)?;
    let conversations = Slab::<Conversation>::worst_case(limits.conversations)?;
    let alarms = Deadlines::<Alarm>::worst_case(limits.runs)?;
    // Each run holds its charter, up to its byte limit, and what it found in
    // its checkout: a guide and a mark for checks per repository.
    let guides = List::<Guide>::worst_case(limits.repositories)?
        .checked_add(u64::from(limits.repositories).checked_mul(u64::from(limits.guide_bytes))?)?;
    let checks = List::<u32>::worst_case(limits.repositories)?;
    // A winding run holds the outcome it accepted.
    let run = limits.run_bytes.checked_add(guides)?.checked_add(checks)?.checked_add(limits.outcome_bytes)?;
    let held = u64::from(limits.runs).checked_mul(run)?;
    // A call landing a change holds it.
    let calls =
        Calls::worst_case(limits.calls)?.checked_add(u64::from(limits.calls).checked_mul(limits.outcome_bytes)?)?;
    runs.checked_add(conversations)?.checked_add(alarms)?.checked_add(held)?.checked_add(calls)
}
