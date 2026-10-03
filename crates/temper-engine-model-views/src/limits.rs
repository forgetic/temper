use temper_lib::{Deadlines, Duration, Id, List, Map, Queue, Slab, Token};

use crate::boundary::{Chunk, Record};
use crate::facts::Fact;
use crate::model::{Op, Run};
use crate::trace::Alarm;
use crate::watch::Watcher;

/// The views sub-model's limits (programming-model.md, section 7), handed by
/// its parent to every step read-only.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    /// Runs followed at once. A run started past them is not followed: what
    /// it reports is dropped and counted, and a watch of it refused. As many
    /// runs turned away are remembered, with their items, to say so.
    pub runs: u32,
    /// Watches open at once. A watch past them is refused as busy.
    pub watchers: u32,
    /// Chunks a watcher's backlog holds while its delivery is in flight.
    /// Past them, what waits is dropped, and the watcher told it missed it.
    pub backlog: u32,
    /// The most bytes a report has. One past it is dropped and counted.
    pub report_bytes: u32,
    /// The most bytes of a watch's snapshot. A watch past it is refused.
    pub snapshot_bytes: u32,
    /// Records a batch holds, and bytes of their content in all: room for a
    /// report at least.
    pub records: u32,
    pub batch_bytes: u32,
    /// Batches on their way to the store at once. Past them, a batch waits,
    /// and a report that finds it full is lost.
    pub appends: u32,
    /// How long a batch waits for more reports before it goes.
    pub flush: Duration,
    /// How long traces are kept, and how often, while the store may hold
    /// some, it is asked to forget what is older.
    pub retention: Duration,
    pub sweep: Duration,
    /// Facts kept until the parent drains them. Beyond them, facts are
    /// dropped and counted.
    pub facts: u32,
}

/// The watchers held at once: each watch open, and as many ended in an
/// iteration, which keep their slots until the reclaim point frees them, so
/// that admission counts the watches open, as the parent sees them.
pub(crate) fn slots(limits: &Limits) -> Option<u32> {
    limits.watchers.checked_mul(2)
}

/// The store operations in flight at once: each append and the expire,
/// twice over, as one may end and the next start in an iteration before the
/// reclaim point frees the first.
pub(crate) fn ops(limits: &Limits) -> Option<u32> {
    limits.appends.checked_add(1)?.checked_mul(2)
}

/// The most memory the model holds under `limits`, in bytes (6.3), or `None`
/// if it does not fit a `u64`, or the limits cannot work: no run, watcher,
/// backlog, record or append, a batch with no room for a report, or no time
/// between sweeps.
///
/// It counts the containers, their bookkeeping included, and the payloads,
/// not allocator overhead: the runs followed, and those turned away; each
/// watch open with a full backlog, and as many ended in an iteration; the
/// batch, with the next one made as it goes; the operations in flight; and
/// what a step holds in hand: a report or a snapshot, a delivery being made
/// of a backlog, and the watchers of a run that finished. What goes out in a
/// request is moved out in the step that makes it, and is its receiver's to
/// count.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    let empty = limits.runs == 0 || limits.watchers == 0 || limits.backlog == 0;
    if empty || limits.records == 0 || limits.appends == 0 || limits.sweep == Duration::ZERO {
        return None;
    }
    if limits.report_bytes > limits.batch_bytes {
        return None;
    }
    let report = u64::from(limits.report_bytes);
    let runs =
        Map::<Token, Run>::worst_case(limits.runs)?.checked_add(Map::<Token, Token>::worst_case(limits.runs)?)?;
    // Each watcher held has its backlog, and those open fill theirs.
    let slots = slots(limits)?;
    let backlogs = u64::from(slots)
        .checked_mul(Queue::<Chunk>::worst_case(limits.backlog)?)?
        .checked_add(u64::from(limits.watchers).checked_mul(u64::from(limits.backlog))?.checked_mul(report)?)?;
    let watchers = Slab::<Watcher>::worst_case(slots)?
        .checked_add(Map::<Token, Id<Watcher>>::worst_case(limits.watchers)?)?
        .checked_add(backlogs)?;
    // A batch holds no more content than its bytes, nor than its records
    // full.
    let content = u64::from(limits.batch_bytes).min(u64::from(limits.records).checked_mul(report)?);
    let batch = List::<Record>::worst_case(limits.records)?.checked_mul(2)?.checked_add(content)?;
    let ops = Slab::<Op>::worst_case(ops(limits)?)?;
    let alarms = Deadlines::<Alarm>::worst_case(2)?;
    let facts = Queue::<Fact>::worst_case(limits.facts)?;
    let hand = report
        .max(u64::from(limits.snapshot_bytes))
        .checked_add(List::<Chunk>::worst_case(limits.backlog)?)?
        .checked_add(List::<Id<Watcher>>::worst_case(limits.watchers)?)?;
    runs.checked_add(watchers)?
        .checked_add(batch)?
        .checked_add(ops)?
        .checked_add(alarms)?
        .checked_add(facts)?
        .checked_add(hand)
}
