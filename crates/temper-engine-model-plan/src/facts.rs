//! What a step's decisions read (engine-model.md, 5.3): what the working set
//! holds about an item and its relations, gathered by the plan's parent from
//! the forge and the record, in the plan's terms. (The plan tells no facts of
//! its own: what it decides is all in what it returns.)
//!
//! Every time here is on the clock of `env.now`: the parent maps the forge's
//! times onto it as it reads them.

use temper_lib::Time;

use crate::plan::Commit;

/// What the forge shows about an item, and what its record says beyond the
/// plan's part.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[expect(clippy::struct_excessive_bools, reason = "independent facts of an item, each read on its own")]
pub struct Facts {
    /// When the item was made.
    pub created: Time,
    /// The steps it comes after, as many as the parent found an item for
    /// among the names its step comes after (a name counted each time it is
    /// named). A step whose dependencies are not all made yet waits.
    pub dependencies: Relations,
    /// The steps it added; on a goal's item, the steps of its plan.
    pub children: Relations,
    /// The head of the item's branch, once a run has pushed a change to it.
    pub branch: Option<Commit>,
    /// Whether that branch is gone from the forge: another party deleted
    /// it, and its pull request, if it had one, closed with it.
    pub gone: bool,
    /// The item's pull request, once it is open.
    pub pull: Option<Pull>,
    /// A person's latest decision on the step itself: on its gate of
    /// acceptance, or on the wait for their decision. (An answer to one of its
    /// proposals is not one: its parent applies the proposal, or tells
    /// [`rejected`](crate::rejected).)
    pub decision: Option<Decided>,
    /// Whether a person closed the item: its step is done, as they said.
    pub closed: bool,
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
    /// When its head was pushed: its waits on the forge count from then.
    pub pushed: Time,
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

/// A person's decision on a step, and when they made it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Decided {
    pub decision: Decision,
    pub at: Time,
}

/// A person's decision on a step: on a wait for one, or on a gate of their
/// acceptance.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Decision {
    Accepted,
    Rejected,
}
