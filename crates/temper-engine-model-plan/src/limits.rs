use temper_lib::{List, Queue};

use crate::check::{Problem, Problems};
use crate::plan::Budget;

/// The plan's limits (section 7), handed by its parent to every call
/// read-only.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    /// Steps a plan holds, the steps it grows by included.
    pub steps: u32,
    /// The most bytes of a name: a step's, a branch's or a template's.
    pub name_bytes: u32,
    /// Steps a step may come after.
    pub dependencies: u32,
    /// Gates a step may add.
    pub gates: u32,
    /// Branches an envelope may let changes land into.
    pub targets: u32,
    /// The most bytes of a charter's instructions.
    pub instruction_bytes: u32,
    /// Tasks one outcome may create.
    pub tasks: u32,
    /// Inbox events a wake decision reads. Beyond them, the rest wait for
    /// the next decision.
    pub events: u32,
    /// Runs that may repair a change before it is held for a person.
    pub repairs: u32,
    /// The largest budget a step may give a run, dimension by dimension.
    pub budget: Budget,
}

/// The most memory one call holds of its own under `limits`, in bytes (6.4),
/// or `None` if it does not fit a `u64` or the limits cannot be honoured: a
/// plan of no steps.
///
/// The plan keeps nothing between calls: what a call holds is the scratch of
/// a plan's checks (a count and a place in the order for each step) and the
/// problems it lists. The writes it asks for, the runs it describes and the
/// problems it returns are handed out, and their receiver counts them.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    if limits.steps == 0 {
        return None;
    }
    // The most steps one check reads: a plan's, or an outcome's tasks.
    let steps = limits.steps.max(limits.tasks);
    let waiting = List::<u32>::worst_case(steps)?;
    let ready = Queue::<u32>::worst_case(steps)?;
    // The problems found, then boxed at their number for the caller.
    let found = List::<Problem>::worst_case(Problems::LISTED)?;
    waiting.checked_add(ready)?.checked_add(found)?.checked_add(found)
}

/// The most writes one call asks for: a plan's items and its goal, or the
/// items it grows by with the goal and the step that grew it, or an outcome's
/// tasks, and room for both writes that finish a change.
#[must_use]
pub fn max_out(limits: &Limits) -> u32 {
    limits.steps.saturating_add(2).max(limits.tasks)
}
