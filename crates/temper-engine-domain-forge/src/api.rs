//! The forge's API as the forge child domain speaks it: the operations it asks
//! the protocol layer for, through its parent, and the answers the protocol
//! layer decodes for it (engine-domain.md, section 13).
//!
//! Names are the forge's own, Forgejo-shaped: an item, an issue or a pull
//! request, by its number in its repository, the two sharing one numbering; a
//! comment by an id that only grows; a user by an opaque id; a commit by its
//! 32 bytes. The repository a call is about travels beside the operation, as
//! the deployment's index for it, which the protocol layer maps to the forge's
//! name for it. Text is bytes, which the domain carries and never reads.
//!
//! What the protocol layer owes these operations:
//!
//! - **Listings** ([`Op::Items`]) are the forge's: the items updated at or
//!   after `since`, at the forge's resolution (a second on Forgejo), least
//!   recently updated first (Forgejo's `sort=leastupdate`: its default is
//!   newest first), a page at a time by number, each saying the forge's time
//!   as it was made (the response's `Date`). An item that changes while a
//!   listing is paged moves to its end; the domain pages by time rather than
//!   by number where it can, and re-lists from the last time it saw, so a
//!   move costs it a duplicate and never a miss.
//! - **Times** the forge shows are its own clock's, which the domain compares
//!   only with one another, never with its own; a rate limit's reset comes
//!   as how long to wait ([`Error::RateLimited`]).
//! - **An item's updated time** moves with new comments, reviews, label
//!   changes, pushes to a pull request's head, closing and reopening, and
//!   edits of its title or body; not with commit statuses, nor with edits of
//!   comments, nor with a move of a pull request's base. So CI and the base
//!   are learnt from webhooks and by reading the pull requests the working
//!   set holds again, and an edited record by reading it afresh before it is
//!   written.
//! - **Pages** of comments are by id; of reviews and statuses, by number,
//!   which the protocol layer maps to Forgejo's. A review a person starts
//!   pending is shown only once submitted, with its earlier id.
//! - **Markers.** A creation's key goes inside what it creates, as a marker
//!   the protocol layer writes and finds again ([`Summary::key`],
//!   [`Mark::Key`]); so does the engine's record, whose inbox position and
//!   the nonce of the write that made it are this child domain's part
//!   ([`Mark::Record`]), and the nonce of the write that made a wiki page
//!   ([`Page::nonce`]). The rest of the record is the parent's, named by a
//!   token ([`Body::Record`]).
//! - **Failures.** A call the protocol layer could not send, or the forge
//!   refused before it read it, fails as [`Error::Unavailable`]: nothing was
//!   done. Any failure after the call may have reached the forge (a server
//!   error, a dropped connection, no answer in time) is an
//!   [`Error::Timeout`]: it may have been done, and may still take effect
//!   until `Limits::lifetime` after it went out.
//! - **Revisions.** A comment's revision changes whenever its body does, so
//!   that an edit is seen when the comment is next read; the answer to a
//!   comment posted or edited says the revision it made.

use alloc::boxed::Box;

use skein_lib::{Duration, Time, Token};

use crate::boundary::{Ci, Position};

/// What a call asks of a repository.
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Op {
    /// A page of the items in `state` (either, if `None`) of `kind` (either,
    /// if `None`) carrying `label` (any, if `None`), opened by `author`
    /// (anyone, if `None`), updated at or after `since`, least recently
    /// updated first: the `page`th, from 1. Answered by [`Answer::Items`].
    Items {
        state: Option<State>,
        kind: Option<Kind>,
        label: Option<Box<[u8]>>,
        author: Option<u64>,
        since: Time,
        page: u32,
    },
    /// The item `number`, and a page of its comments with ids above `after`,
    /// oldest first. Answered by [`Answer::Item`].
    Item {
        number: u64,
        after: u64,
    },
    /// The comment `id` on the item `number`. Answered by
    /// [`Answer::Comment`].
    Comment {
        number: u64,
        id: u64,
    },
    /// The pull request `number`: its head, where its base is, and CI on its
    /// head. Answered by [`Answer::Pull`].
    Pull {
        number: u64,
    },
    /// The newest pull request, open or not, that merges `head` into `base`.
    /// Answered by [`Answer::Pull`].
    PullFor {
        head: Box<[u8]>,
        base: Box<[u8]>,
    },
    /// The `page`th page (from 1) of the reviews of the pull request
    /// `number`, oldest first. Answered by [`Answer::Reviews`].
    Reviews {
        number: u64,
        page: u32,
    },
    /// CI on `commit`, and the `page`th page (from 1) of the latest status of
    /// each context on it. Answered by [`Answer::Statuses`].
    Statuses {
        commit: [u8; 32],
        page: u32,
    },
    /// The `page`th page (from 1) of the inline comments of the review
    /// `review` of the pull request `number`. Answered by
    /// [`Answer::Remarks`].
    Remarks {
        number: u64,
        review: u64,
        page: u32,
    },
    /// The permission of `user`. Answered by [`Answer::Permission`].
    Permission {
        user: u64,
    },
    /// Where `branch` is. Answered by [`Answer::Commit`].
    Branch {
        branch: Box<[u8]>,
    },
    /// A page of the wiki's page names, in order, from after `after`.
    /// Answered by [`Answer::Pages`].
    Pages {
        after: Option<Box<[u8]>>,
    },
    /// The wiki page `name`. Answered by [`Answer::Page`].
    Page {
        name: Box<[u8]>,
    },
    /// Opens an issue carrying `labels`, with `key` inside it. Answered by
    /// [`Answer::Created`].
    CreateIssue {
        key: Box<[u8]>,
        title: Box<[u8]>,
        body: Body,
        labels: Box<[Box<[u8]>]>,
    },
    /// Comments on the item `number`, with `key` inside the comment if there
    /// is one (a record carries none: it is found as the engine's record),
    /// and the person it is written for, if it is a person's (a message
    /// from the web). Answered by [`Answer::Commented`].
    Post {
        number: u64,
        key: Option<Box<[u8]>>,
        person: Option<u64>,
        body: Body,
    },
    /// Edits the comment `id` on the item `number`. Answered by
    /// [`Answer::Edited`].
    EditComment {
        number: u64,
        id: u64,
        body: Body,
    },
    /// Adds `labels` to the item `number`, leaving those it carries. Answered
    /// by [`Answer::Done`].
    AddLabels {
        number: u64,
        labels: Box<[Box<[u8]>]>,
    },
    /// Removes `labels` from the item `number`, those it does not carry
    /// aside. Answered by [`Answer::Done`].
    RemoveLabels {
        number: u64,
        labels: Box<[Box<[u8]>]>,
    },
    /// Opens a pull request to merge `head` into `base`. Answered by
    /// [`Answer::Created`].
    OpenPull {
        title: Box<[u8]>,
        body: Body,
        head: Box<[u8]>,
        base: Box<[u8]>,
    },
    /// Merges the pull request `number` if its head is still `head`.
    /// Answered by [`Answer::Merged`].
    Merge {
        number: u64,
        head: [u8; 32],
    },
    /// Reviews the pull request `number` at its head with `verdict`, with
    /// `key` inside the review. Answered by [`Answer::Reviewed`].
    Review {
        number: u64,
        key: Box<[u8]>,
        verdict: Verdict,
        body: Body,
    },
    /// Makes the users asked to review the pull request `number` exactly
    /// `reviewers`. Answered by [`Answer::Done`].
    SetReviewers {
        number: u64,
        reviewers: Box<[u64]>,
    },
    /// Makes the items of its repository the item `number` depends on
    /// exactly `dependencies`. Answered by [`Answer::Done`].
    SetDependencies {
        number: u64,
        dependencies: Box<[u64]>,
    },
    /// Closes the item `number`; reopens it. Answered by [`Answer::Done`].
    Close {
        number: u64,
    },
    Reopen {
        number: u64,
    },
    /// Deletes `branch`. Answered by [`Answer::Done`].
    DeleteBranch {
        branch: Box<[u8]>,
    },
    /// Creates or replaces the wiki page `name`, with `nonce` inside it.
    /// Answered by [`Answer::Revision`].
    PutPage {
        name: Box<[u8]>,
        content: Body,
        nonce: u64,
    },
    /// Deletes the wiki page `name`. Answered by [`Answer::Done`].
    DeletePage {
        name: Box<[u8]>,
    },
}

/// What a write carries: bytes the domain carries as they are, or a payload
/// its parent names and fills in as the call goes out (programming-model.md,
/// 4.2).
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Body {
    Text(Box<[u8]>),
    Payload(Token),
    /// The engine's record: the parent's part, named by `payload`, and this
    /// child domain's: the inbox position, and the nonce of the write.
    Record {
        payload: Token,
        position: Position,
        nonce: u64,
    },
}

/// The answer to a call that succeeded.
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Answer {
    /// A page of items, whether another may follow it, and the forge's time
    /// as it made the page.
    Items {
        items: Box<[Summary]>,
        more: bool,
        now: Time,
    },
    /// An item and a page of its comments, and whether more follow.
    Item {
        item: Summary,
        comments: Box<[Comment]>,
        more: bool,
    },
    Comment(Comment),
    Pull(Pull),
    /// A page of reviews, and whether more follow.
    Reviews {
        reviews: Box<[Review]>,
        more: bool,
    },
    /// CI on a commit, over all its contexts; a page of their statuses, in
    /// their contexts' order, and whether more follow.
    Statuses {
        ci: Ci,
        statuses: Box<[Status]>,
        more: bool,
    },
    /// A page of a review's inline comments, and whether more follow.
    Remarks {
        remarks: Box<[Remark]>,
        more: bool,
    },
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
    /// The id of the review made.
    Reviewed(u64),
    /// The commit the merge made on the base.
    Merged([u8; 32]),
    /// The wiki page's revision after the write.
    Revision(u64),
    Done,
}

/// Why a call failed.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Error {
    /// The protocol layer could not send it, or the forge refused it before
    /// reading it: nothing was done. Worth retrying.
    Unavailable,
    /// It may have reached the forge, and failed after (a server error, a
    /// dropped connection, no answer in time): what was asked may have been
    /// done, or may still be.
    Timeout,
    /// Too many calls: none is taken for `after`, until the reset the forge
    /// named.
    RateLimited { after: Duration },
    /// The engine's permission does not allow it.
    Forbidden,
    /// What the call names is not there: the item, comment, pull request,
    /// branch or page.
    Missing,
    /// Beyond the forge's limits.
    TooLarge,
    /// A title or a body the forge requires is empty.
    Empty,
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
    /// An item would depend on itself, or on one that depends on it.
    Circular,
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
    /// When it was posted, the forge's time: what a write it causes names
    /// ([`crate::Cause`]).
    pub created: Time,
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
    /// A creation's key, and the person the engine wrote it for, if it is a
    /// person's: a message from the web, news from them.
    Key { key: Box<[u8]>, person: Option<u64> },
    /// A record, and this child domain's part of it: the inbox position, and
    /// the nonce of the write that made it.
    Record { position: Position, nonce: u64 },
    /// A record that does not decode.
    Mangled,
}

/// A pull request: its branches, its head commit, where its base branch is,
/// whether and how it merged, whether it merges cleanly, and CI on its head,
/// over all its contexts.
#[derive(PartialEq, Eq, Hash, Debug)]
pub struct Pull {
    pub number: u64,
    pub state: State,
    pub head: Box<[u8]>,
    pub base: Box<[u8]>,
    pub commit: [u8; 32],
    pub base_commit: Option<[u8; 32]>,
    pub merged: Option<[u8; 32]>,
    pub mergeable: bool,
    pub ci: Ci,
}

#[derive(PartialEq, Eq, Hash, Debug)]
pub struct Review {
    /// An id that only grows, across the forge.
    pub id: u64,
    pub author: u64,
    pub verdict: Verdict,
    /// The head it reviewed.
    pub commit: [u8; 32],
    /// The key found inside it, if the engine made it.
    pub key: Option<Box<[u8]>>,
    pub body: Box<[u8]>,
}

/// A review's inline comment: on the line `line` of the file `path`.
#[derive(PartialEq, Eq, Hash, Debug)]
pub struct Remark {
    pub id: u64,
    pub author: u64,
    pub path: Box<[u8]>,
    pub line: u32,
    pub body: Box<[u8]>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Verdict {
    Approve,
    RequestChanges,
    Comment,
}

/// A context's status on a commit: what it says of itself, and where its
/// output is (what a brief of a CI failure shows).
#[derive(PartialEq, Eq, Hash, Debug)]
pub struct Status {
    pub context: Box<[u8]>,
    pub check: Check,
    pub description: Box<[u8]>,
    pub url: Box<[u8]>,
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
    /// The nonce of the engine's write that made this revision, if one did.
    pub nonce: Option<u64>,
}

#[derive(PartialEq, Eq, Hash, Debug)]
pub struct PageName {
    pub name: Box<[u8]>,
    pub revision: u64,
}

// What a call is answered with, by the operation it asked: any other answer
// is the protocol layer's bug.

/// The answer to [`Op::Items`].
pub(crate) fn items(answer: Answer) -> (Box<[Summary]>, bool, Time) {
    match answer {
        Answer::Items { items, more, now } => (items, more, now),
        Answer::Item { .. }
        | Answer::Comment(_)
        | Answer::Pull(_)
        | Answer::Reviews { .. }
        | Answer::Statuses { .. }
        | Answer::Remarks { .. }
        | Answer::Permission(_)
        | Answer::Commit(_)
        | Answer::Pages { .. }
        | Answer::Page(_)
        | Answer::Created(_)
        | Answer::Commented { .. }
        | Answer::Edited { .. }
        | Answer::Reviewed(_)
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
        | Answer::Reviews { .. }
        | Answer::Statuses { .. }
        | Answer::Remarks { .. }
        | Answer::Permission(_)
        | Answer::Commit(_)
        | Answer::Pages { .. }
        | Answer::Page(_)
        | Answer::Created(_)
        | Answer::Commented { .. }
        | Answer::Edited { .. }
        | Answer::Reviewed(_)
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
        | Answer::Reviews { .. }
        | Answer::Statuses { .. }
        | Answer::Remarks { .. }
        | Answer::Permission(_)
        | Answer::Commit(_)
        | Answer::Pages { .. }
        | Answer::Page(_)
        | Answer::Created(_)
        | Answer::Commented { .. }
        | Answer::Edited { .. }
        | Answer::Reviewed(_)
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
        | Answer::Reviews { .. }
        | Answer::Statuses { .. }
        | Answer::Remarks { .. }
        | Answer::Permission(_)
        | Answer::Commit(_)
        | Answer::Pages { .. }
        | Answer::Page(_)
        | Answer::Created(_)
        | Answer::Commented { .. }
        | Answer::Edited { .. }
        | Answer::Reviewed(_)
        | Answer::Merged(_)
        | Answer::Revision(_)
        | Answer::Done => unreachable!("a pull request's read is answered with the pull request"),
    }
}

/// The answer to [`Op::Page`].
pub(crate) fn page(answer: Answer) -> Page {
    match answer {
        Answer::Page(page) => page,
        Answer::Items { .. }
        | Answer::Item { .. }
        | Answer::Comment(_)
        | Answer::Pull(_)
        | Answer::Reviews { .. }
        | Answer::Statuses { .. }
        | Answer::Remarks { .. }
        | Answer::Permission(_)
        | Answer::Commit(_)
        | Answer::Pages { .. }
        | Answer::Created(_)
        | Answer::Commented { .. }
        | Answer::Edited { .. }
        | Answer::Reviewed(_)
        | Answer::Merged(_)
        | Answer::Revision(_)
        | Answer::Done => unreachable!("a wiki page's read is answered with the page"),
    }
}

/// The answer to [`Op::Reviews`].
pub(crate) fn reviews(answer: Answer) -> (Box<[Review]>, bool) {
    match answer {
        Answer::Reviews { reviews, more } => (reviews, more),
        Answer::Items { .. }
        | Answer::Item { .. }
        | Answer::Comment(_)
        | Answer::Pull(_)
        | Answer::Statuses { .. }
        | Answer::Remarks { .. }
        | Answer::Permission(_)
        | Answer::Commit(_)
        | Answer::Pages { .. }
        | Answer::Page(_)
        | Answer::Created(_)
        | Answer::Commented { .. }
        | Answer::Edited { .. }
        | Answer::Reviewed(_)
        | Answer::Merged(_)
        | Answer::Revision(_)
        | Answer::Done => unreachable!("a pull request's reviews are answered with reviews"),
    }
}
