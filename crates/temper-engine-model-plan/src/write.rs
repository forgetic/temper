//! The writes the plan asks for, in its own terms (engine-model.md, 4.4 and
//! 4.5). Its parent checks each against the rules, and makes them through the
//! forge, keyed, serialised per item; the record's update comes last, and is
//! the commit point. A write is about the item the decision was asked for,
//! unless it says otherwise.

use alloc::boxed::Box;

use crate::plan::Commit;
use crate::record::{Goal, Progress, Record};

/// A write, in the plan's terms.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Write {
    /// Make an item for a step: an issue in the step's repository, titled
    /// with its name, carrying the record's part given here. An item the
    /// outcome made is the child of the item the decision was asked for.
    Create { key: Key, record: Box<Record> },
    /// Open the pull request of the item's change, from the item's branch
    /// into `base`. It is keyed by the branch.
    OpenPull { base: Box<[u8]> },
    /// Open again the item's pull request, which was closed unmerged.
    ReopenPull,
    /// Merge the item's pull request at exactly `head`, or not at all.
    Merge { head: Commit },
    /// Close the item: its step is done.
    Close,
    /// Delete the item's branch, once its change has landed.
    DeleteBranch,
    /// The step's progress, in the item's record, becomes this.
    Progress(Progress),
    /// The goal's part of its item's record becomes this: on the item itself
    /// when its outcome proposed the plan, on the goal's item when a step under
    /// it grew it.
    Goal(Goal),
    /// Release the step of the item's goal named `step`, as a person would:
    /// its parent asks [`release`](crate::release) what that writes.
    Release { step: Box<[u8]> },
}

/// What a creation is keyed by, after the outcome that causes it: its parent
/// puts the outcome's own identity (its item and attempt) before it, so a
/// second application finds what the first one made.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Key {
    /// A step's item, by the step's name.
    Step(Box<[u8]>),
    /// A task's item, by its place among the outcome's tasks.
    Task(u32),
}
