//! The forge's API, as its model layer sees it once the protocol layer has
//! parsed a request: the subset of Forgejo's that temper uses, and the git a
//! worker speaks to it.
//!
//! Names are the forge's own. A repository is named by its full name
//! (`owner/name`), and every call is about one; an item, an issue or a pull
//! request, by its number in its repository, the two sharing one numbering; a
//! comment by an id that only grows, across the forge; a user by an id; a
//! commit by its count in the forge's one store, so a seed replays to the same
//! names. Text is bytes, compared byte for byte.
//!
//! Creating carries no key: a client that must find what it created puts its
//! own marker in a body, and reads.

use alloc::boxed::Box;

use temper_lib::{Duration, Time};

/// A user's permission on a repository. Each grants what the ones before it
/// do.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Permission {
    None,
    /// Read, open issues and pull requests, comment and review.
    Read,
    /// Label, merge, push, delete branches, edit the wiki, report statuses.
    Write,
    /// Edit and delete anyone's comments.
    Admin,
}

/// What a call asks of a repository.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Op {
    Read(Read),
    Write(Write),
    Git(Git),
}

/// The reads: they change nothing. Each needs read permission.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Read {
    /// A page of the items in `state` (either, if `None`) of `kind` (either,
    /// if `None`) that carry every label of `labels` and were updated at or
    /// after `since`, in the order they were last updated, from after
    /// `after`. Answered by [`Answer::Items`].
    Items { state: Option<State>, kind: Option<Kind>, labels: Box<[Box<[u8]>]>, since: Time, after: Option<Cursor> },
    /// The item `number` and a page of its comments with ids above `after`.
    /// Answered by [`Answer::Item`].
    Item { number: u64, after: u64 },
    /// The pull request `number`, its reviews and the statuses on its head.
    /// Answered by [`Answer::Pull`].
    Pull { number: u64 },
    /// The permission of `user`. Answered by [`Answer::Permission`].
    Permission { user: u64 },
    /// Where `branch` is. Answered by [`Answer::Commit`].
    Branch { branch: Box<[u8]> },
    /// The files of `commit`, which the repository has. Answered by
    /// [`Answer::Tree`].
    Tree { commit: u64 },
    /// One file of `commit`. Answered by [`Answer::File`].
    File { commit: u64, path: Box<[u8]> },
    /// A page of the wiki's page names, in their order, from after `after`.
    /// Answered by [`Answer::Pages`].
    Pages { after: Option<Box<[u8]>> },
    /// The wiki page `name`. Answered by [`Answer::Page`].
    Page { name: Box<[u8]> },
}

/// The writes, each answered by [`Answer::Done`] unless it says otherwise.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Write {
    /// Opens an issue carrying `labels`, which are defined. Answered by
    /// [`Answer::Created`] with its number. Labelling needs write permission.
    CreateIssue {
        title: Box<[u8]>,
        body: Box<[u8]>,
        labels: Box<[Box<[u8]>]>,
    },
    /// Comments on the item `number`. Answered by [`Answer::Commented`] with
    /// the comment's id.
    Comment {
        number: u64,
        body: Box<[u8]>,
    },
    /// Edits the comment `id`: its author's, or anyone's for an admin.
    EditComment {
        id: u64,
        body: Box<[u8]>,
    },
    /// Deletes the comment `id`: its author's, or anyone's for an admin.
    DeleteComment {
        id: u64,
    },
    /// Makes the labels of the item `number` exactly `labels`, which are
    /// defined. Needs write permission.
    SetLabels {
        number: u64,
        labels: Box<[Box<[u8]>]>,
    },
    /// Defines the label `name`. Needs write permission.
    DefineLabel {
        name: Box<[u8]>,
    },
    /// Opens a pull request to merge `head` into `base`. Answered by
    /// [`Answer::Created`] with its number.
    OpenPull {
        title: Box<[u8]>,
        body: Box<[u8]>,
        head: Box<[u8]>,
        base: Box<[u8]>,
    },
    /// Reviews the open pull request `number` at its head.
    Review {
        number: u64,
        verdict: Verdict,
        body: Box<[u8]>,
    },
    /// Merges the pull request `number` if its head is still `head`, as one
    /// commit on its base. Answered by [`Answer::Merged`]. Needs write
    /// permission.
    Merge {
        number: u64,
        head: u64,
    },
    /// Closes or reopens the item `number`: its author's, or anyone's with
    /// write permission.
    Close {
        number: u64,
    },
    Reopen {
        number: u64,
    },
    /// Deletes `branch`, which is neither the default nor protected. Needs
    /// write permission.
    DeleteBranch {
        branch: Box<[u8]>,
    },
    /// Reports the status `state` of `context` on `commit`, which the
    /// repository has. Needs write permission.
    Status {
        commit: u64,
        context: Box<[u8]>,
        state: Check,
    },
    /// Creates or replaces the wiki page `name`. Answered by
    /// [`Answer::Revision`]. Needs write permission.
    PutPage {
        name: Box<[u8]>,
        content: Box<[u8]>,
    },
    /// Deletes the wiki page `name`. Needs write permission.
    DeletePage {
        name: Box<[u8]>,
    },
}

/// What git does against the forge, the remote side. Fetching needs read
/// permission; pushing and creating, write.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Git {
    /// Every branch, and the default. Answered by [`Answer::Cloned`].
    Clone,
    /// What `want` names, and the commits before it. Answered by
    /// [`Answer::Commit`].
    Fetch { want: Want },
    /// Moves `branch` to `commit`, a commit of the store, as a fast-forward,
    /// or creates it. Answered by [`Answer::Pushed`].
    Push { branch: Box<[u8]>, commit: u64 },
    /// Creates `branch` at `commit`, which the repository has, only if it is
    /// nowhere. Answered by [`Answer::Branch`].
    Create { branch: Box<[u8]>, commit: u64 },
}

/// What a fetch asks for.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Want {
    Branch(Box<[u8]>),
    Commit(u64),
    Default,
}

/// The answer to a call that succeeded.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Answer {
    /// A page of items, and where the next starts if there may be more.
    Items {
        items: Box<[Summary]>,
        next: Option<Cursor>,
    },
    /// An item and a page of its comments, oldest first, and whether more
    /// follow.
    Item {
        item: Summary,
        comments: Box<[Comment]>,
        more: bool,
    },
    Pull(Pull),
    Permission(Permission),
    /// Where a branch is, or what a fetch fetched.
    Commit(u64),
    Tree(Box<[File]>),
    File(Box<[u8]>),
    /// A page of wiki page names, and the last of them if there may be more.
    Pages {
        pages: Box<[PageName]>,
        next: Option<Box<[u8]>>,
    },
    Page(Page),
    /// The number of the item opened.
    Created(u64),
    /// The id of the comment posted.
    Commented(u64),
    /// The commit the merge made on the base.
    Merged(u64),
    /// The wiki page's new revision.
    Revision(u64),
    Done,
    Cloned {
        default: Box<[u8]>,
        branches: Box<[Head]>,
    },
    Pushed(Pushed),
    Branch(Created),
}

/// Why a call failed.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Error {
    /// The forge failed, or was too busy: nothing was done. Worth retrying.
    Unavailable,
    /// The forge timed out answering: what was asked may have been done.
    Timeout,
    /// The user made too many calls: none is taken until `reset`.
    RateLimited {
        reset: Time,
    },
    /// The user's permission does not allow it, or the comment or item is
    /// someone else's.
    Forbidden,
    Missing(What),
    /// A name, title, body, content, tree or label list beyond the forge's
    /// limits.
    TooLarge,
    /// The repository holds as much of it as it may.
    Full,
    /// A label of that name is defined, or an open pull request has the same
    /// head and base.
    Exists,
    /// The head has nothing the base does not have.
    NothingToMerge,
    /// The pull request is closed, or merged.
    Closed,
    /// The pull request's head is no longer the one named.
    Stale,
    /// The head and the base changed a path differently.
    Conflict,
    /// The branch's protection refuses it: the default branch, or a merge
    /// lacking the statuses or approvals it requires, or a push or deletion.
    Protected,
    /// The repository cannot be reached.
    Unreachable,
    /// The repository refuses what is pushed to it.
    Refused,
}

/// What the forge does not have.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum What {
    Repository,
    Item,
    /// The item is an issue, not a pull request.
    Pull,
    Comment,
    Label,
    Branch,
    Commit,
    File,
    Page,
}

/// Whether an item is an issue or a pull request.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Kind {
    Issue,
    Pull,
}

/// Whether an item is open. A merged pull request is closed.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum State {
    Open,
    Closed,
}

/// Where a page of items ends: the next page starts after it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Cursor {
    pub updated: Time,
    pub number: u64,
}

/// An item as a listing shows it.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Summary {
    pub number: u64,
    pub kind: Kind,
    pub state: State,
    pub title: Box<[u8]>,
    pub body: Box<[u8]>,
    pub author: u64,
    /// In their order.
    pub labels: Box<[Box<[u8]>]>,
    pub created: Time,
    /// Moves on every change to the item: its state, labels, comments,
    /// reviews, head, and the statuses on its head.
    pub updated: Time,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Comment {
    pub id: u64,
    pub author: u64,
    pub body: Box<[u8]>,
    pub created: Time,
    /// When it was last edited.
    pub edited: Option<Time>,
}

/// A pull request: its branches, its head commit (which follows the head
/// branch while it is open), whether and how it merged, its reviews oldest
/// first, and the latest status of each context on its head.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Pull {
    pub number: u64,
    pub state: State,
    pub head: Box<[u8]>,
    pub base: Box<[u8]>,
    pub commit: u64,
    /// Where the base branch is, unless it was deleted.
    pub base_commit: Option<u64>,
    /// The commit its merge made.
    pub merged: Option<u64>,
    /// Whether the head merges into the base without a conflict.
    pub mergeable: bool,
    pub reviews: Box<[Review]>,
    pub statuses: Box<[Status]>,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Review {
    pub author: u64,
    pub verdict: Verdict,
    /// The head it reviewed.
    pub commit: u64,
    pub body: Box<[u8]>,
    pub at: Time,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Verdict {
    Approve,
    RequestChanges,
    Comment,
}

/// A context's status on a commit.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Status {
    pub context: Box<[u8]>,
    pub state: Check,
    pub author: u64,
    pub at: Time,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Check {
    Pending,
    Passed,
    Failed,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct File {
    pub path: Box<[u8]>,
    pub content: Box<[u8]>,
}

/// A wiki page. Its revision grows with every write to the repository's wiki.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Page {
    pub name: Box<[u8]>,
    pub content: Box<[u8]>,
    pub author: u64,
    pub revision: u64,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct PageName {
    pub name: Box<[u8]>,
    pub revision: u64,
}

/// A branch and where it is.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Head {
    pub branch: Box<[u8]>,
    pub commit: u64,
}

/// How a push went: the branch is at the commit, or it is not an ancestor of
/// it and was left where it is.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Pushed {
    Pushed,
    Rejected,
}

/// How a creation went: the branch was made, or it existed and was left where
/// it is.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Created {
    Created,
    Exists,
}

/// What a webhook says changed. Those about an item name it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Change {
    /// An issue opened, closed, reopened or labelled.
    Issue,
    /// A pull request opened, closed, reopened, labelled or merged, or its
    /// head moved.
    Pull,
    /// A comment posted, edited or deleted.
    Comment,
    Review,
    /// A status reported on a commit.
    Status,
    /// A branch moved, made or deleted.
    Push,
    /// A wiki page written or deleted.
    Wiki,
}

// A scenario: what a world sets the forge up with.

/// A repository to add: its name, its default branch at a first commit of
/// `tree`, its labels, its CI, its protected branch, and whether a subscriber
/// hears of its changes.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Setup {
    pub name: Box<[u8]>,
    pub default: Box<[u8]>,
    pub tree: Box<[File]>,
    pub labels: Box<[Box<[u8]>]>,
    pub checks: Checks,
    pub protection: Option<Protection>,
    pub hooked: bool,
}

/// A repository's CI: the contexts it reports on every commit that becomes a
/// branch's or a pull request's head, pending at once and then passed or
/// failed after a latency drawn from `latency_min..=latency_max`, or never.
/// Chances are per mille.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Checks {
    pub contexts: Box<[Box<[u8]>]>,
    pub latency_min: Duration,
    pub latency_max: Duration,
    /// The chance that a context never reports past pending.
    pub silent: u32,
    /// The chance that a context passes, of those that report.
    pub passes: u32,
    /// When set, content decides instead of `passes`.
    pub cue: Option<Cue>,
}

/// A verdict cued by content: CI passes on a commit whose file at `path`
/// holds `green`, and fails on any other.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Cue {
    pub path: Box<[u8]>,
    pub green: Box<[u8]>,
}

/// A protected branch: nothing is pushed to it or deletes it, and a pull
/// request merges into it only with each of `contexts` passed on its exact
/// head and `approvals` approving reviews of that head by users with write
/// permission, its author aside.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Protection {
    pub branch: Box<[u8]>,
    pub contexts: Box<[Box<[u8]>]>,
    pub approvals: u32,
}
