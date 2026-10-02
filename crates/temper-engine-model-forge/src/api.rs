//! The forge's API as the forge sub-model speaks it: the operations it asks
//! the protocol layer for, through its parent, and the answers the protocol
//! layer decodes for it (engine-model.md, section 13).
//!
//! Names are the forge's own, Forgejo-shaped: an item, an issue or a pull
//! request, by its number in its repository, the two sharing one numbering; a
//! comment by an id that only grows; a user by an opaque id; a commit by its
//! 32 bytes. The repository a call is about travels beside the operation, as
//! the deployment's index for it, which the protocol layer maps to the forge's
//! name for it. Text is bytes, which the model carries and never reads.
//!
//! What the protocol layer owes these operations:
//!
//! - **Listings** ([`Op::Items`]) are the forge's: the items updated at or
//!   after `since`, at the forge's resolution (a second on Forgejo), least
//!   recently updated first, a page at a time by number. An item that changes
//!   while a listing is paged moves to its end; the model pages by time
//!   rather than by number where it can, and re-lists from the last time it
//!   saw, so a move costs it a duplicate and never a miss.
//! - **An item's updated time** moves with new comments, reviews, label
//!   changes, pushes to a pull request's head, closing and reopening, and
//!   edits of its title or body; not with commit statuses, nor with edits of
//!   comments. So CI is learnt from status webhooks and by reading the
//!   statuses on the heads the working set holds, and an edited record by
//!   reading it afresh before it is written.
//! - **Markers.** A creation's key goes inside what it creates, as a marker
//!   the protocol layer writes and finds again ([`Summary::key`],
//!   [`Mark::Key`]); so does the engine's record, whose inbox position is this
//!   sub-model's part ([`Mark::Record`]). The rest of the record is the
//!   parent's, named by a token ([`Body::Record`]).
//! - **Revisions.** A comment's revision changes whenever its body does, so
//!   that an edit is seen when the comment is next read; the answer to a
//!   comment posted or edited says the revision it made.

use alloc::boxed::Box;

use temper_lib::{Time, Token};

use crate::boundary::Position;

/// What a call asks of a repository.
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Op {
    /// A page of the items in `state` (either, if `None`) of `kind` (either,
    /// if `None`) carrying `label` (any, if `None`), updated at or after
    /// `since`, least recently updated first: the `page`th, from 1. Answered
    /// by [`Answer::Items`].
    Items { state: Option<State>, kind: Option<Kind>, label: Option<Box<[u8]>>, since: Time, page: u32 },
    /// The item `number`, and a page of its comments with ids above `after`,
    /// oldest first. Answered by [`Answer::Item`].
    Item { number: u64, after: u64 },
    /// The comment `id` on the item `number`. Answered by
    /// [`Answer::Comment`].
    Comment { number: u64, id: u64 },
    /// The pull request `number`: its head, its reviews, and the statuses on
    /// its head. Answered by [`Answer::Pull`].
    Pull { number: u64 },
    /// The newest pull request, open or not, that merges `head` into `base`.
    /// Answered by [`Answer::Pull`].
    PullFor { head: Box<[u8]>, base: Box<[u8]> },
    /// The latest status of each context on `commit`. Answered by
    /// [`Answer::Statuses`].
    Statuses { commit: [u8; 32] },
    /// The permission of `user`. Answered by [`Answer::Permission`].
    Permission { user: u64 },
    /// Where `branch` is. Answered by [`Answer::Commit`].
    Branch { branch: Box<[u8]> },
    /// A page of the wiki's page names, in order, from after `after`.
    /// Answered by [`Answer::Pages`].
    Pages { after: Option<Box<[u8]>> },
    /// The wiki page `name`. Answered by [`Answer::Page`].
    Page { name: Box<[u8]> },
    /// Opens an issue carrying `labels`, with `key` inside it. Answered by
    /// [`Answer::Created`].
    CreateIssue { key: Box<[u8]>, title: Box<[u8]>, body: Body, labels: Box<[Box<[u8]>]> },
    /// Comments on the item `number`, with `key` inside the comment if there
    /// is one (a record carries none: it is found as the engine's record).
    /// Answered by [`Answer::Commented`].
    Post { number: u64, key: Option<Box<[u8]>>, body: Body },
    /// Edits the comment `id` on the item `number`. Answered by
    /// [`Answer::Edited`].
    EditComment { number: u64, id: u64, body: Body },
    /// Makes the labels of the item `number` exactly `labels`. Answered by
    /// [`Answer::Done`].
    SetLabels { number: u64, labels: Box<[Box<[u8]>]> },
    /// Opens a pull request to merge `head` into `base`. Answered by
    /// [`Answer::Created`].
    OpenPull { title: Box<[u8]>, body: Body, head: Box<[u8]>, base: Box<[u8]> },
    /// Merges the pull request `number` if its head is still `head`.
    /// Answered by [`Answer::Merged`].
    Merge { number: u64, head: [u8; 32] },
    /// Closes the item `number`. Answered by [`Answer::Done`].
    Close { number: u64 },
    /// Deletes `branch`. Answered by [`Answer::Done`].
    DeleteBranch { branch: Box<[u8]> },
    /// Creates or replaces the wiki page `name`. Answered by
    /// [`Answer::Revision`].
    PutPage { name: Box<[u8]>, content: Body },
    /// Deletes the wiki page `name`. Answered by [`Answer::Done`].
    DeletePage { name: Box<[u8]> },
}

/// What a write carries: bytes the model carries as they are, or a payload
/// its parent names and fills in as the call goes out (programming-style.md,
/// 4.2).
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Body {
    Text(Box<[u8]>),
    Payload(Token),
    /// The engine's record: the parent's part, named by `payload`, and the
    /// inbox position, this sub-model's.
    Record {
        payload: Token,
        position: Position,
    },
}

/// The answer to a call that succeeded.
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Answer {
    /// A page of items, and whether another may follow it.
    Items {
        items: Box<[Summary]>,
        more: bool,
    },
    /// An item and a page of its comments, and whether more follow.
    Item {
        item: Summary,
        comments: Box<[Comment]>,
        more: bool,
    },
    Comment(Comment),
    Pull(Pull),
    /// In their contexts' order.
    Statuses(Box<[Status]>),
    Permission(Permission),
    /// Where a branch is.
    Commit([u8; 32]),
    /// A page of wiki page names, and the last of them if more may follow.
    Pages {
        pages: Box<[PageName]>,
        next: Option<Box<[u8]>>,
    },
    Page(Page),
    /// The number of the item opened.
    Created(u64),
    /// The comment posted, and its revision.
    Commented {
        id: u64,
        revision: u64,
    },
    /// The comment's revision after the edit.
    Edited {
        revision: u64,
    },
    /// The commit the merge made on the base.
    Merged([u8; 32]),
    /// The wiki page's revision after the write.
    Revision(u64),
    Done,
}

/// Why a call failed.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Error {
    /// The forge failed, or the protocol layer could not send it: nothing was
    /// done. Worth retrying.
    Unavailable,
    /// No answer in time: what was asked may have been done.
    Timeout,
    /// Too many calls: none is taken until `reset`, the forge's time.
    RateLimited { reset: Time },
    /// The engine's permission does not allow it.
    Forbidden,
    /// What the call names is not there: the item, comment, pull request,
    /// branch or page.
    Missing,
    /// Beyond the forge's limits.
    TooLarge,
    /// The repository holds as much of it as it may.
    Full,
    /// An open pull request has the same head and base.
    Exists,
    /// The head has nothing the base does not.
    NothingToMerge,
    /// The pull request is closed, or merged.
    Closed,
    /// The pull request's head is no longer the one named.
    Stale,
    /// The head and the base conflict.
    Conflict,
    /// The branch's protection refuses it.
    Protected,
}

/// A user's permission on a repository.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Permission {
    None,
    Read,
    Write,
    Admin,
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

/// An item as a listing or a read shows it.
#[derive(PartialEq, Eq, Hash, Debug)]
pub struct Summary {
    pub number: u64,
    pub kind: Kind,
    pub state: State,
    pub author: u64,
    /// The key found inside it, if the engine created it.
    pub key: Option<Box<[u8]>>,
    pub labels: Box<[Box<[u8]>]>,
    pub title: Box<[u8]>,
    pub body: Box<[u8]>,
    pub updated: Time,
}

#[derive(PartialEq, Eq, Hash, Debug)]
pub struct Comment {
    pub id: u64,
    pub author: u64,
    /// Changes whenever the body does.
    pub revision: u64,
    pub mark: Mark,
    pub body: Box<[u8]>,
}

/// What the protocol layer found inside a comment.
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Mark {
    /// Nothing of the engine's.
    None,
    /// A creation's key.
    Key(Box<[u8]>),
    /// A record, and this sub-model's part of it.
    Record(Position),
    /// A record that does not decode.
    Mangled,
}

/// A pull request: its branches, its head commit, whether and how it
/// merged, its reviews oldest first, and the latest status of each context on
/// its head.
#[derive(PartialEq, Eq, Hash, Debug)]
pub struct Pull {
    pub number: u64,
    pub state: State,
    pub head: Box<[u8]>,
    pub base: Box<[u8]>,
    pub commit: [u8; 32],
    pub merged: Option<[u8; 32]>,
    pub mergeable: bool,
    pub reviews: Box<[Review]>,
    pub statuses: Box<[Status]>,
}

#[derive(PartialEq, Eq, Hash, Debug)]
pub struct Review {
    pub author: u64,
    pub verdict: Verdict,
    /// The head it reviewed.
    pub commit: [u8; 32],
    pub body: Box<[u8]>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Verdict {
    Approve,
    RequestChanges,
    Comment,
}

/// A context's status on a commit.
#[derive(PartialEq, Eq, Hash, Debug)]
pub struct Status {
    pub context: Box<[u8]>,
    pub check: Check,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Check {
    Pending,
    Passed,
    Failed,
}

#[derive(PartialEq, Eq, Hash, Debug)]
pub struct Page {
    pub name: Box<[u8]>,
    pub content: Box<[u8]>,
    pub revision: u64,
}

#[derive(PartialEq, Eq, Hash, Debug)]
pub struct PageName {
    pub name: Box<[u8]>,
    pub revision: u64,
}

// What a call is answered with, by the operation it asked: any other answer
// is the protocol layer's bug.

/// The answer to [`Op::Items`].
pub(crate) fn items(answer: Answer) -> (Box<[Summary]>, bool) {
    match answer {
        Answer::Items { items, more } => (items, more),
        Answer::Item { .. }
        | Answer::Comment(_)
        | Answer::Pull(_)
        | Answer::Statuses(_)
        | Answer::Permission(_)
        | Answer::Commit(_)
        | Answer::Pages { .. }
        | Answer::Page(_)
        | Answer::Created(_)
        | Answer::Commented { .. }
        | Answer::Edited { .. }
        | Answer::Merged(_)
        | Answer::Revision(_)
        | Answer::Done => unreachable!("a listing is answered with items"),
    }
}

/// The answer to [`Op::Item`].
pub(crate) fn item(answer: Answer) -> (Summary, Box<[Comment]>, bool) {
    match answer {
        Answer::Item { item, comments, more } => (item, comments, more),
        Answer::Items { .. }
        | Answer::Comment(_)
        | Answer::Pull(_)
        | Answer::Statuses(_)
        | Answer::Permission(_)
        | Answer::Commit(_)
        | Answer::Pages { .. }
        | Answer::Page(_)
        | Answer::Created(_)
        | Answer::Commented { .. }
        | Answer::Edited { .. }
        | Answer::Merged(_)
        | Answer::Revision(_)
        | Answer::Done => unreachable!("an item's read is answered with the item"),
    }
}

/// The answer to [`Op::Comment`].
pub(crate) fn comment(answer: Answer) -> Comment {
    match answer {
        Answer::Comment(comment) => comment,
        Answer::Items { .. }
        | Answer::Item { .. }
        | Answer::Pull(_)
        | Answer::Statuses(_)
        | Answer::Permission(_)
        | Answer::Commit(_)
        | Answer::Pages { .. }
        | Answer::Page(_)
        | Answer::Created(_)
        | Answer::Commented { .. }
        | Answer::Edited { .. }
        | Answer::Merged(_)
        | Answer::Revision(_)
        | Answer::Done => unreachable!("a comment's read is answered with the comment"),
    }
}

/// The answer to [`Op::Pull`] and [`Op::PullFor`].
pub(crate) fn pull(answer: Answer) -> Pull {
    match answer {
        Answer::Pull(pull) => pull,
        Answer::Items { .. }
        | Answer::Item { .. }
        | Answer::Comment(_)
        | Answer::Statuses(_)
        | Answer::Permission(_)
        | Answer::Commit(_)
        | Answer::Pages { .. }
        | Answer::Page(_)
        | Answer::Created(_)
        | Answer::Commented { .. }
        | Answer::Edited { .. }
        | Answer::Merged(_)
        | Answer::Revision(_)
        | Answer::Done => unreachable!("a pull request's read is answered with the pull request"),
    }
}
