//! The records that cross the boundary with the forge sub-model's parent, the
//! engine's top-level model (programming-style.md, 4.5). The forge sub-model
//! defines them; its parent depends on it.
//!
//! It has two faces, both through its parent:
//!
//! - The forge's, which the parent translates to and from the protocol
//!   layer. A [`Request::Call`] is ended by exactly one [`Event::Answered`],
//!   echoing its `call`. Webhooks come up as [`Event::Hint`]: hints, which
//!   make it look sooner and are never relied on.
//! - The parent's own. A [`Event::Read`] is answered by exactly one
//!   [`Request::Read`], and a [`Event::Write`] by exactly one
//!   [`Request::Wrote`], each echoing its `owner`; a [`Event::Track`] by a
//!   [`Request::Announced`] once the item is read, by a [`Request::Left`] if
//!   it is closed or not there, or at once by a [`Request::Full`]. A write
//!   that fails as [`Failure::Forge`] with [`Error::Timeout`] may have been
//!   made: the parent asks for it again, `resumed`, if it still wants it.
//!   Everything else it tells unasked, as the forge changes: an item handed
//!   in ([`Request::Offered`]), news for an item's inbox
//!   ([`Request::Inbox`]), its labels changing ([`Request::Changed`]), its
//!   leaving ([`Request::Left`]), an item the forge keeps refusing to show
//!   ([`Request::Forbidden`]), room in the working set after a refusal
//!   ([`Request::Room`]), and the end of the cold start
//!   ([`Request::Loaded`]).
//!
//! What an item's dependencies are, and whether they finished, the parent
//! reads afresh: the working set holds the items the engine tracks, and a
//! dependency that is not one leaves no news.
//!
//! Items, repositories and people are named as the deployment names them
//! (an [`Item`], a repository's index in the deployment's list, a person's
//! forge user id); payloads the sub-model does not interpret are named by
//! the parent's tokens ([`Content::Payload`]), and filled in by the parent
//! as the call that carries them goes out.

use alloc::boxed::Box;

use temper_lib::{Time, Token};

use crate::api::{Answer, Error, Kind, Op, Verdict};

/// An issue or pull request of one of the deployment's repositories: the
/// repository's index in the deployment's list, and the item's number there.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Item {
    pub repository: u32,
    pub number: u64,
}

/// parent -> forge
#[derive(PartialEq, Eq, Debug)]
pub enum Event {
    /// Hold `item` in the working set: the engine created it, or takes it in
    /// (engine-model.md, 4.6). Its record is read, and it is announced; or it
    /// leaves at once, if it is closed or not there; or it is refused, if
    /// there is no room. An item held already changes nothing.
    Track { item: Item },
    /// Stop holding `item`, which the engine no longer tracks. Nothing is
    /// told of it after.
    Untrack { item: Item },
    /// The pull request `pull` of `item`'s repository carries `item`'s change,
    /// or none does: its head, CI and reviews are now `item`'s.
    Link { item: Item, pull: Option<u64> },
    /// The parent took `item`'s news up to and including `through`: the
    /// inbox position moves past them, and the next record written carries it.
    Took { item: Item, through: u64 },
    /// A webhook: something changed in `repository`, about the item `item`,
    /// the commit `commit` (a status, a push) or the branch `branch` (a
    /// push) if it names one.
    Hint { repository: u32, item: Option<u64>, commit: Option<[u8; 32]>, branch: Option<Box<[u8]>> },
    /// A fresh read, answered by one [`Request::Read`].
    Read { owner: Token, read: Read },
    /// A write, answered by one [`Request::Wrote`]. `resumed` names its
    /// cause when it may have been asked for before: by an engine that has
    /// since restarted, or by this one before the write failed as
    /// [`Failure::Forge`] with [`Error::Timeout`]. A creation is then looked
    /// for by its key, among what came after its cause, before it is tried.
    Write { owner: Token, write: Write, resumed: Option<Cause> },
    /// Terminal for [`Request::Call`]: what the forge answered, or why it did
    /// not.
    Answered { call: Token, result: Result<Answer, Error> },
}

/// forge -> parent
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    /// To the forge: do `op` on `repository`. Ended by one
    /// [`Event::Answered`].
    Call { call: Token, repository: u32, op: Op },
    /// The answer to a [`Event::Read`]: exactly one per read.
    Read { owner: Token, result: Result<Answer, Failure> },
    /// The answer to a [`Event::Write`]: exactly one per write.
    Wrote { owner: Token, result: Result<Written, Failure> },
    /// `item` was not taken in: the working set is full. One carrying the
    /// tracking label is found again by a listing once there is room; for
    /// the rest, the parent hears [`Request::Room`], and asks again.
    Full { item: Item },
    /// The working set has room again, after a [`Request::Full`].
    Room,
    /// `item` is in the working set, read: what it is and its record.
    Announced { item: Item, view: View },
    /// `item`, an open issue, carries the hand-in label and is not tracked.
    Offered { item: Item },
    /// News for `item`'s inbox, the `seq`th since it was announced.
    Inbox { item: Item, seq: u64, news: News },
    /// `item`'s labels are now `labels`.
    Changed { item: Item, labels: Box<[Box<[u8]>]> },
    /// `item` left the working set, as it is closed, or not on the forge
    /// (deleted, or moved to another repository).
    Left { item: Item, why: Why },
    /// The forge refused the engine `item` as many times in a row as the
    /// limits' `attempts`: it stays in the working set, read again after
    /// each backoff, and its news follows once the forge shows it again.
    Forbidden { item: Item },
    /// The cold start is done: every item that carried the tracking label as
    /// it began has been read and announced, or did not fit.
    Loaded,
}

/// What caused a write, as the forge knows it: the comment `comment` (an
/// outcome, a person's request), posted at `at`, the forge's time. What the
/// write creates comes after it: a comment has a greater id (ids grow across
/// the forge), an issue an updated time no earlier.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Cause {
    pub comment: u64,
    pub at: Time,
}

/// A fresh read, of the forge as it is now (engine-model.md, 4.4): fetched
/// within the request budget, ahead of everything else, and not kept.
#[derive(PartialEq, Eq, Debug)]
pub enum Read {
    /// The item and a page of its comments with ids above `after`.
    Item {
        item: Item,
        after: u64,
    },
    /// The pull request `item`: its head, where its base is, and CI on its
    /// head.
    Pull {
        item: Item,
    },
    /// The `page`th page (from 1) of the reviews of the pull request `item`.
    Reviews {
        item: Item,
        page: u32,
    },
    /// The `page`th page (from 1) of the inline comments of the review
    /// `review` of the pull request `item`.
    Remarks {
        item: Item,
        review: u64,
        page: u32,
    },
    PullFor {
        repository: u32,
        head: Box<[u8]>,
        base: Box<[u8]>,
    },
    /// CI on `commit`, and the `page`th page (from 1) of its statuses.
    Statuses {
        repository: u32,
        commit: [u8; 32],
        page: u32,
    },
    Permission {
        repository: u32,
        user: u64,
    },
    Branch {
        repository: u32,
        branch: Box<[u8]>,
    },
    Pages {
        repository: u32,
        after: Option<Box<[u8]>>,
    },
    Page {
        repository: u32,
        name: Box<[u8]>,
    },
}

/// A write (seams: "Outcomes and writes"), serialised with the others about
/// the same item, retried when it fails for a while, and found by its key
/// when an attempt may or may not have been made.
#[derive(PartialEq, Eq, Debug)]
pub enum Write {
    /// Opens an issue in `repository`, keyed. Done as [`Written::Created`].
    CreateIssue {
        repository: u32,
        key: Box<[u8]>,
        title: Box<[u8]>,
        body: Content,
        labels: Box<[Box<[u8]>]>,
    },
    /// Comments on `item`, keyed; for `person`, if it is a person's message
    /// the engine writes for them, whose news it is when it is read. Done as
    /// [`Written::Commented`].
    Comment {
        item: Item,
        key: Box<[u8]>,
        person: Option<u64>,
        body: Content,
    },
    /// Writes `item`'s record: the parent's part named by `payload`, and the
    /// inbox position taken. Posted if the item has none, edited otherwise,
    /// once a fresh read finds it as last read: a record someone else changed
    /// is not written over ([`Failure::Edited`]). Done as [`Written::Done`].
    Record {
        item: Item,
        payload: Token,
    },
    /// Makes `item`'s labels among those the engine owns
    /// ([`crate::Config`]) exactly `labels`, which it owns: they are added,
    /// and the others it owns removed, while the labels people set stay as
    /// they are. Done as [`Written::Done`].
    SetLabels {
        item: Item,
        labels: Box<[Box<[u8]>]>,
    },
    /// Opens a pull request in `repository` to merge `head` into `base`,
    /// keyed by its branches. Done as [`Written::Created`].
    OpenPull {
        repository: u32,
        title: Box<[u8]>,
        body: Content,
        head: Box<[u8]>,
        base: Box<[u8]>,
    },
    /// Merges the pull request `item` at exactly `head`. Done as
    /// [`Written::Merged`].
    Merge {
        item: Item,
        head: [u8; 32],
    },
    /// Reviews the pull request `item` at its head with `verdict`, keyed (a
    /// verdict an outcome gives). Done as [`Written::Reviewed`].
    Review {
        item: Item,
        key: Box<[u8]>,
        verdict: Verdict,
        body: Content,
    },
    /// Makes the users asked to review the pull request `item` exactly
    /// `reviewers`: a set. Done as [`Written::Done`].
    SetReviewers {
        item: Item,
        reviewers: Box<[u64]>,
    },
    /// Makes the items of its repository `item` depends on exactly
    /// `dependencies`: a set. Done as [`Written::Done`].
    SetDependencies {
        item: Item,
        dependencies: Box<[u64]>,
    },
    /// Closes `item`; reopens it. Done as [`Written::Done`].
    Close {
        item: Item,
    },
    Reopen {
        item: Item,
    },
    /// Deletes `branch` of `repository`. Done as [`Written::Done`].
    DeleteBranch {
        repository: u32,
        branch: Box<[u8]>,
    },
    /// Writes the wiki page `name` of `repository`, once a fresh read finds
    /// it at `revision` as last read, or finds none if `None`: a page someone
    /// else wrote or deleted since is not written over
    /// ([`Failure::Revised`]). Done as [`Written::Revision`].
    PutPage {
        repository: u32,
        name: Box<[u8]>,
        content: Content,
        revision: Option<u64>,
    },
    /// Deletes the wiki page `name` of `repository`. Done as
    /// [`Written::Done`].
    DeletePage {
        repository: u32,
        name: Box<[u8]>,
    },
}

/// What a write carries: bytes, or a payload the parent names, which it
/// fills in as the call goes out.
#[derive(PartialEq, Eq, Debug)]
pub enum Content {
    Text(Box<[u8]>),
    Payload(Token),
}

/// How a write went.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Written {
    /// The item opened, by this write or by an earlier attempt found by its
    /// key.
    Created(u64),
    /// The comment posted, likewise.
    Commented(u64),
    /// The commit the merge made.
    Merged([u8; 32]),
    /// The review made, by this write or by an earlier attempt found by its
    /// key.
    Reviewed(u64),
    /// The wiki page's revision.
    Revision(u64),
    Done,
}

/// Why a read or a write was not done.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Failure {
    /// Refused at the entrance: as many reads or writes are in hand as the
    /// limits allow.
    Busy,
    /// Refused at the entrance: beyond the limits, or naming a repository the
    /// deployment does not have.
    Invalid,
    /// A record written for an item whose record is not known: it is not in
    /// the working set, or not read yet.
    Unknown,
    /// A record that someone else changed, or deleted, since it was last
    /// read: it is now `record`, and was not written over.
    Edited { record: Record },
    /// A wiki page that someone else wrote, or deleted, since it was last
    /// read: it is now at `revision`, or gone if `None`, and was not written
    /// over.
    Revised { revision: Option<u64> },
    /// The forge refused it, or kept failing until the attempts ran out.
    Forge(Error),
}

/// What decisions need of an item, as it was read.
#[derive(PartialEq, Eq, Debug)]
pub struct View {
    pub kind: Kind,
    pub labels: Box<[Box<[u8]>]>,
    pub record: Record,
}

/// An item's record, as this sub-model knows it: where it is, and the inbox
/// position it says. The rest of it is the parent's.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Record {
    /// The item has none.
    Missing,
    /// The comment `comment` holds a record that does not decode: the item is
    /// held for a person (seams: "The record").
    Mangled {
        comment: u64,
        revision: u64,
    },
    Found {
        comment: u64,
        revision: u64,
        position: Position,
    },
}

/// What an item has taken of its inbox (engine-model.md, 4.3): the last
/// comment on it and on its pull request, the verdicts on its pull request's
/// head (a digest of them), and the head and CI seen.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Position {
    pub comment: u64,
    pub pull_comment: u64,
    pub reviews: u64,
    pub head: Option<[u8; 32]>,
    pub ci: Ci,
}

impl Position {
    /// Nothing taken.
    pub const START: Position = Position { comment: 0, pull_comment: 0, reviews: 0, head: None, ci: Ci::None };
}

/// Why an item left the working set.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Why {
    /// The forge shows it closed: done.
    Closed,
    /// The forge does not have it: deleted, or moved to another repository.
    Missing,
}

/// CI on a head, over its contexts: failed if any failed, pending if any is
/// pending, passed if all passed, none if none reported.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Ci {
    None,
    Pending,
    Passed,
    Failed,
}

/// Something new for an item's inbox, with what decisions need to know of it
/// (who, which comment, which state); content is fetched on demand.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum News {
    /// A person commented: the comment `id`, by `author`, on the item `on`:
    /// this one, or its pull request.
    Comment { on: u64, id: u64, author: u64 },
    /// The verdicts on the item's pull request's head `commit` changed: the
    /// working set holds them ([`crate::Model::reviews`]).
    Reviews { commit: [u8; 32] },
    /// The item's pull request moved: its head, CI on that head, whether it is
    /// open, merged, and merges cleanly.
    Pull { commit: [u8; 32], ci: Ci, open: bool, merged: Option<[u8; 32]>, mergeable: bool },
}

/// An item's pull request as last read: what a step's decisions read of a
/// change (seams: "Plans"), on its exact head, and where its base is.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Level {
    pub number: u64,
    pub commit: [u8; 32],
    pub base: Option<[u8; 32]>,
    pub ci: Ci,
    pub open: bool,
    pub merged: Option<[u8; 32]>,
    pub mergeable: bool,
}

/// A reviewer's latest verdict on a pull request's head: approve, or request
/// changes (a review that only comments is no verdict).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Reviewed {
    pub author: u64,
    pub verdict: Verdict,
}
