//! The plan's parts of an item's record (engine-domain.md, 4.1), which its
//! parent composes with the other parts when it writes the record and splits
//! from them when it reads one: the step the item carries and how far it has
//! come; on a goal's item, the goal's plan.

use alloc::boxed::Box;

use skein_lib::Time;

use crate::due::Why;
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

/// What a step has done that the forge does not show, and what its decisions
/// need to survive a restart: the run claimed for it, what the outcomes
/// applied to it changed, and a person's last release of it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Progress {
    /// Its runs are done: an agent step's report or growth, or a session's
    /// last turn, applied.
    pub finished: bool,
    /// Why the run claimed for it runs, while one is: [`due`](crate::due)
    /// writes it with the claim, and applying the run's outcome clears it.
    pub running: Option<Why>,
    /// When its last run was claimed: a session's last turn, which its wake
    /// rule's timer counts from.
    pub last_run: Option<Time>,
    /// The runs claimed for it so far, each claim counting itself: what
    /// tells the steps its runs added to its goal apart.
    pub runs: u32,
    /// Runs that repaired a change for a failure (CI failed, changes asked
    /// for), their outcomes applied, since it was made or last released.
    pub repairs: u32,
    /// Runs that rebased a change onto a base that moved (conflicting with it
    /// or not), their outcomes applied, since it was made or last released.
    pub rebases: u32,
    /// Its proposals a person rejected (growth beyond its goal's envelope, a
    /// plan), since it was made or last released.
    pub rejections: u32,
    /// A change's last review by an agent.
    pub review: Option<Reviewed>,
    /// When a person last released it: decisions on it made before then
    /// count for nothing, and its waits on the forge count from then.
    pub released: Option<Time>,
}

impl Progress {
    /// A step's, as its item is made.
    pub const NEW: Progress = Progress {
        finished: false,
        running: None,
        last_run: None,
        runs: 0,
        repairs: 0,
        rebases: 0,
        rejections: 0,
        review: None,
        released: None,
    };
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
    /// The run of the step, or of the goal's session, whose outcome added it,
    /// by the count of runs claimed for it: what finds a growth already
    /// applied, when it is applied again.
    pub run: u32,
}
