//! The plan's parts of an item's record (engine-model.md, 4.1), which its
//! parent composes with the other parts when it writes the record and splits
//! from them when it reads one: the step the item carries and how far it has
//! come; on a goal's item, the goal's plan.

use alloc::boxed::Box;

use crate::plan::{Commit, Envelope, Growth, Step};

/// The plan's part of an item's record.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Record {
    pub step: Step,
    pub progress: Progress,
    /// On a goal's item, its plan: the item whose outcome proposed the plan
    /// becomes its goal once the plan is accepted.
    pub goal: Option<Goal>,
}

/// What a step has done that the forge does not show: what the outcomes
/// applied to it so far changed.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Progress {
    /// An agent step's run has finished: its report, or the steps it added,
    /// applied.
    pub finished: bool,
    /// Runs that repaired a change for a failure (CI failed, changes asked
    /// for, a conflict), their outcomes applied. Rebasing onto a base that
    /// moved is not counted.
    pub repairs: u32,
    /// A change's last review by an agent.
    pub review: Option<Reviewed>,
}

impl Progress {
    /// A step's, as its item is made.
    pub const NEW: Progress = Progress { finished: false, repairs: 0, review: None };
}

/// An agent's verdict on a change, and the exact head it was given on.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Reviewed {
    pub head: Commit,
    pub verdict: Verdict,
}

/// A review's verdict on a change's head.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Verdict {
    Approve,
    /// Changes are asked for: a run repairs the change.
    Changes,
}

/// An accepted plan, on its goal's item: what its growth is checked against.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Goal {
    /// Its steps, in the order they joined it.
    pub steps: Box<[Entry]>,
    pub envelope: Envelope,
    pub budget: u64,
    /// The tokens its steps are estimated to spend.
    pub estimate: u64,
    /// The steps added since it was accepted, of each primitive.
    pub growth: Growth,
}

/// A step of an accepted plan, as its goal keeps it: enough to check that
/// growth makes no cycle.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Entry {
    pub name: Box<[u8]>,
    /// The steps it comes after.
    pub after: Box<[Box<[u8]>]>,
    /// The step that added it, by its place among the goal's steps, if a step
    /// did: that step is done only once this one is.
    pub parent: Option<u32>,
}
