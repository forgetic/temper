use alloc::boxed::Box;

use temper_lib::{Id, Map, Queue, Set, Slab, Token};

use crate::boundary::Reason;
use crate::call::Call;
use crate::facts::Fact;
use crate::hosted::Hosted;

/// The host sub-model's limits (section 7), handed by its parent to every step
/// read-only.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    /// Runs hosted at once: the run slots. An assignment beyond them is
    /// refused as busy.
    pub slots: u32,
    /// Repositories a workspace may list.
    pub repositories: u32,
    /// The most bytes of a workstream key, a repository name or remote, a
    /// branch or an identity.
    pub name_bytes: u32,
    /// The most bytes of a charter.
    pub charter_bytes: u64,
    /// The most bytes of a snapshot: an assignment's, or a parked run's.
    pub snapshot_bytes: u64,
    /// The most bytes of a run's declared outcome. A run that says more has
    /// broken the rules.
    pub outcome_bytes: u64,
    /// The most bytes of detail an answer carries for operators: the tail of
    /// what it is given.
    pub detail_bytes: u32,
    /// Inbound events a run holds until it is live. Beyond them, an event is
    /// bounced.
    pub held: u32,
    /// The most bytes of an inbound event.
    pub event_bytes: u64,
    /// Host calls a run may have in flight at once, its push among them. A
    /// call beyond them is answered as busy.
    pub run_calls: u32,
    /// Facts kept until the parent drains them. Beyond them, facts are
    /// dropped and counted.
    pub facts: u32,
}

/// The host calls in flight at once, across runs, under `limits`, or `None` if
/// they do not fit a `u32`.
pub(crate) fn calls(limits: &Limits) -> Option<u32> {
    limits.slots.checked_mul(limits.run_calls)
}

/// The most memory the model holds under `limits`, in bytes (6.4), or `None`
/// if it does not fit a `u64`.
///
/// It counts the containers, their bookkeeping included, and the payloads, not
/// allocator overhead. What the host passes on (a workspace, the bodies of
/// host calls and their answers, what a push or a save did) is moved into a
/// request in the step it arrives in, and what it makes for a request (an
/// answer, a report) is moved out in the step that makes it: either is its
/// receiver's to count.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    let hosted = Slab::<Hosted>::worst_case(limits.slots)?;
    let names = Map::<Token, Id<Hosted>>::worst_case(limits.slots)?;
    let ready = Map::<Id<Hosted>, Reason>::worst_case(limits.slots)?;
    let calls = Slab::<Call>::worst_case(calls(limits)?)?;
    let facts = Queue::<Fact>::worst_case(limits.facts)?;
    // Until its agent starts, a run holds its charter, its snapshot and the
    // inbound events that came meanwhile; from when it is told how the run
    // finishes, the outcome, the snapshot or the detail of the failure.
    let held = Queue::<Box<[u8]>>::worst_case(limits.held)?
        .checked_add(u64::from(limits.held).checked_mul(limits.event_bytes)?)?;
    let starting = limits.charter_bytes.checked_add(limits.snapshot_bytes)?.checked_add(held)?;
    let ending = limits.outcome_bytes.max(limits.snapshot_bytes).max(u64::from(limits.detail_bytes));
    // Throughout, the saved-work branch, the repositories its pushes landed
    // in with the last commit landed in each, and its relayed calls in flight
    // (its push is held inline).
    let landed = Map::<u32, [u8; 32]>::worst_case(limits.repositories)?;
    let run_calls = Set::<Id<Call>>::worst_case(limits.run_calls)?;
    let run =
        starting.max(ending).checked_add(u64::from(limits.name_bytes))?.checked_add(landed)?.checked_add(run_calls)?;
    let runs = u64::from(limits.slots).checked_mul(run)?;
    hosted.checked_add(names)?.checked_add(ready)?.checked_add(calls)?.checked_add(facts)?.checked_add(runs)
}
