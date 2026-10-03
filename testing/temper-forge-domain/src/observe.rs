//! What happened on the forge, content and all, for a referee
//! (testing-strategy.md, section 7): every change a call, CI or another party
//! made, in a bounded queue a world drains at its own pace.
//!
//! A refused write or git call is observed too, so that a referee sees what
//! a client tried that the forge kept from happening: a merge its protection
//! refused, a stale head, a push that was not a fast-forward.
//!
//! Observations are outside the boundary: they are not requests, take no room
//! in `out`, and when the queue is full they are dropped and counted. Nothing
//! the forge does depends on whether one was kept. A world's setup is not
//! observed.

use alloc::boxed::Box;

use skein_lib::Queue;

use crate::api::{Check, Error, Git, Kind, Op, Verdict, Write};

/// A change on the forge, in `repository`, made by the user `by`.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Observation {
    /// `branch` moved from `from` to `to`, or was made at `to` if it was
    /// nowhere.
    Moved {
        repository: Box<[u8]>,
        branch: Box<[u8]>,
        from: Option<u64>,
        to: u64,
        by: u64,
    },
    /// `branch`, which was at `at`, was deleted.
    Deleted {
        repository: Box<[u8]>,
        branch: Box<[u8]>,
        at: u64,
        by: u64,
    },
    /// The item `number` was opened, carrying `labels`; a pull request, to
    /// merge its `branches`.
    Opened {
        repository: Box<[u8]>,
        number: u64,
        kind: Kind,
        title: Box<[u8]>,
        body: Box<[u8]>,
        labels: Box<[Box<[u8]>]>,
        branches: Option<Branches>,
        by: u64,
    },
    Closed {
        repository: Box<[u8]>,
        number: u64,
        by: u64,
    },
    Reopened {
        repository: Box<[u8]>,
        number: u64,
        by: u64,
    },
    /// The item `number`'s labels became `labels`.
    Labelled {
        repository: Box<[u8]>,
        number: u64,
        labels: Box<[Box<[u8]>]>,
        by: u64,
    },
    /// The item `number` now has `title` and `body`.
    Revised {
        repository: Box<[u8]>,
        number: u64,
        title: Box<[u8]>,
        body: Box<[u8]>,
        by: u64,
    },
    /// The item `number` now depends on `dependencies`.
    Depends {
        repository: Box<[u8]>,
        number: u64,
        dependencies: Box<[u64]>,
        by: u64,
    },
    /// The users asked to review the pull request `number` are now
    /// `reviewers`.
    Requested {
        repository: Box<[u8]>,
        number: u64,
        reviewers: Box<[u64]>,
        by: u64,
    },
    /// The label `label` was defined.
    Defined {
        repository: Box<[u8]>,
        label: Box<[u8]>,
        by: u64,
    },
    /// The comment `id` was posted on the item `number`.
    Commented {
        repository: Box<[u8]>,
        number: u64,
        id: u64,
        body: Box<[u8]>,
        by: u64,
    },
    /// The comment `id` on the item `number` now says `body`.
    Edited {
        repository: Box<[u8]>,
        number: u64,
        id: u64,
        body: Box<[u8]>,
        by: u64,
    },
    /// The comment `id` on the item `number` was deleted.
    Removed {
        repository: Box<[u8]>,
        number: u64,
        id: u64,
        by: u64,
    },
    /// The pull request `number` was reviewed at `commit`, by the review
    /// `id`: as it was made, or, if it was pending, as it was submitted.
    Reviewed {
        repository: Box<[u8]>,
        number: u64,
        id: u64,
        commit: u64,
        verdict: Verdict,
        body: Box<[u8]>,
        by: u64,
    },
    /// `context` on `commit` is `state`.
    Reported {
        repository: Box<[u8]>,
        commit: u64,
        context: Box<[u8]>,
        state: Check,
        by: u64,
    },
    /// The pull request `number` merged its head `head` as `commit` on its
    /// base `base`.
    Merged {
        repository: Box<[u8]>,
        number: u64,
        base: Box<[u8]>,
        head: u64,
        commit: u64,
        by: u64,
    },
    /// A write or a git call was refused for `error`, and changed nothing:
    /// `what` it was, the item `number` and the `commit` it named, if it
    /// named them. Reads, and calls on a repository the forge does not
    /// have, are not observed.
    Refused {
        repository: Box<[u8]>,
        what: Operation,
        number: Option<u64>,
        commit: Option<u64>,
        error: Error,
        by: u64,
    },
    /// A push of `commit` to `branch`, which is not its ancestor, was
    /// rejected, leaving the branch where it was.
    Rejected {
        repository: Box<[u8]>,
        branch: Box<[u8]>,
        commit: u64,
        by: u64,
    },
    /// The wiki page `name` now holds `content`, at `revision`, or was
    /// deleted.
    Wiki {
        repository: Box<[u8]>,
        name: Box<[u8]>,
        content: Option<Box<[u8]>>,
        revision: u64,
        by: u64,
    },
}

/// What a pull request opened merges: its head branch, at `commit`, into its
/// base.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Branches {
    pub head: Box<[u8]>,
    pub base: Box<[u8]>,
    pub commit: u64,
}

/// Which write or git call was refused.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Operation {
    CreateIssue,
    EditItem,
    SetDependencies,
    Comment,
    EditComment,
    DeleteComment,
    SetLabels,
    AddLabels,
    RemoveLabels,
    DefineLabel,
    OpenPull,
    SetReviewers,
    Review,
    Submit,
    Merge,
    Close,
    Reopen,
    DeleteBranch,
    Status,
    PutPage,
    DeletePage,
    Clone,
    Fetch,
    Push,
    Create,
}

/// Which write or git call `op` is, and the item and commit it names; `None`
/// for a read.
pub(crate) fn subject(op: &Op) -> Option<(Operation, Option<u64>, Option<u64>)> {
    let subject = match op {
        Op::Read(_) => return None,
        Op::Write(write) => match write {
            Write::CreateIssue { .. } => (Operation::CreateIssue, None, None),
            Write::EditItem { number, .. } => (Operation::EditItem, Some(*number), None),
            Write::SetDependencies { number, .. } => (Operation::SetDependencies, Some(*number), None),
            Write::Comment { number, .. } => (Operation::Comment, Some(*number), None),
            Write::EditComment { .. } => (Operation::EditComment, None, None),
            Write::DeleteComment { .. } => (Operation::DeleteComment, None, None),
            Write::SetLabels { number, .. } => (Operation::SetLabels, Some(*number), None),
            Write::AddLabels { number, .. } => (Operation::AddLabels, Some(*number), None),
            Write::RemoveLabels { number, .. } => (Operation::RemoveLabels, Some(*number), None),
            Write::DefineLabel { .. } => (Operation::DefineLabel, None, None),
            Write::OpenPull { .. } => (Operation::OpenPull, None, None),
            Write::SetReviewers { number, .. } => (Operation::SetReviewers, Some(*number), None),
            Write::Review { number, .. } => (Operation::Review, Some(*number), None),
            Write::Submit { number, .. } => (Operation::Submit, Some(*number), None),
            Write::Merge { number, head } => (Operation::Merge, Some(*number), Some(*head)),
            Write::Close { number } => (Operation::Close, Some(*number), None),
            Write::Reopen { number } => (Operation::Reopen, Some(*number), None),
            Write::DeleteBranch { .. } => (Operation::DeleteBranch, None, None),
            Write::Status { commit, .. } => (Operation::Status, None, Some(*commit)),
            Write::PutPage { .. } => (Operation::PutPage, None, None),
            Write::DeletePage { .. } => (Operation::DeletePage, None, None),
        },
        Op::Git(git) => match git {
            Git::Clone => (Operation::Clone, None, None),
            Git::Fetch { .. } => (Operation::Fetch, None, None),
            Git::Push { commit, .. } => (Operation::Push, None, Some(*commit)),
            Git::Create { commit, .. } => (Operation::Create, None, Some(*commit)),
        },
    };
    Some(subject)
}

/// The observations not yet drained, and how many did not fit.
#[derive(Debug)]
pub(crate) struct Observations {
    queue: Queue<Observation>,
    lost: u64,
}

impl Observations {
    pub(crate) fn with_capacity(capacity: u32) -> Observations {
        Observations { queue: Queue::with_capacity(capacity), lost: 0 }
    }

    /// Keeps `observation` if there is room for it, and counts it otherwise.
    pub(crate) fn push(&mut self, observation: Observation) {
        if self.queue.try_push(observation).is_err() {
            self.lost = self.lost.saturating_add(1);
        }
    }

    pub(crate) fn pop(&mut self) -> Option<Observation> {
        self.queue.pop()
    }

    pub(crate) fn lost(&self) -> u64 {
        self.lost
    }
}
