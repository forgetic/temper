//! What a step's decisions read (engine-model.md, 5.3): what the working set
//! holds about an item and its relations, gathered by the plan's parent from
//! the forge and the record, in the plan's terms. (The plan tells no facts of
//! its own: what it decides is all in what it returns.)

use temper_lib::Time;

use crate::plan::Commit;

/// What the forge shows about an item, and what its record says beyond the
/// plan's part.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Facts {
    /// When the item was made.
    pub created: Time,
    /// The steps it comes after, and the steps each of them added.
    pub dependencies: Relations,
    /// The steps it added; on a goal's item, the steps of its plan.
    pub children: Relations,
    /// The head of the item's branch, once a run has pushed a change to it.
    pub branch: Option<Commit>,
    /// The item's pull request, once it is open.
    pub pull: Option<Pull>,
    /// A person's latest decision on the item.
    pub decision: Option<Decision>,
    /// Whether the engine holds the snapshot of a run that parked.
    pub snapshot: bool,
    /// Whether the item's inbox holds events its wake rule lets through now
    /// ([`wake`](crate::wake)).
    pub woken: bool,
}

/// How far some of the item's relations have come.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Relations {
    pub total: u32,
    pub done: u32,
    /// When the last of those done was done.
    pub last_done: Option<Time>,
}

impl Relations {
    pub const NONE: Relations = Relations { total: 0, done: 0, last_done: None };

    /// Whether every one of them is done, which none is too.
    #[must_use]
    pub fn all_done(self) -> bool {
        self.done >= self.total
    }
}

/// A change's pull request, as the forge shows it now.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Pull {
    pub head: Commit,
    pub state: PullState,
    /// CI on its exact head.
    pub ci: Ci,
    /// People who approve its exact head.
    pub approvals: u32,
    /// Whether anyone asks for changes on its exact head.
    pub changes_requested: bool,
    pub merge: Mergeable,
    /// Whether its base has moved since its head was made on it.
    pub base_moved: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PullState {
    Open,
    Merged,
    /// Closed without being merged.
    Closed,
}

/// CI on a head.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Ci {
    /// Nothing reported yet.
    None,
    Pending,
    Passed,
    Failed,
}

/// Whether a pull request merges into its base cleanly.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Mergeable {
    /// The forge has not said yet.
    Unknown,
    Clean,
    Conflicts,
}

/// A person's decision on an item: on a wait for one, or on a gate of their
/// acceptance.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Decision {
    Accepted,
    Rejected,
}
