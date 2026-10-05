//! The records that cross the boundary with the checkout's parent, the
//! worker's root domain (4.5), which routes them to and from the
//! checkout's clients and the protocol layer. The checkout defines them; its
//! parent depends on it.
//!
//! Two shapes cross it. A client's hold on a workspace: an [`Event::Prepare`]
//! is answered by exactly one [`Request::Prepared`], after a
//! [`Request::Held`] that names the hold if it was admitted; the client
//! addresses the hold by that name until an [`Event::Release`], answered by
//! exactly one [`Request::Released`], and may meanwhile push or save, each
//! answered by exactly one [`Request::Pushed`] or [`Request::Saved`]. Every
//! record back carries the client's token (4.2). And operations out with
//! exactly one terminal event in: an [`Request::Io`] is ended by exactly one
//! [`Event::Done`], after a [`Request::Cancel`] too, and its `owner`, the
//! hold's token, is echoed on it.
//!
//! A step ends at most one of a client's prepares, pushes and saves (one
//! `Prepared`, `Pushed` or `Saved`), beside the `Held` of a prepare it
//! admits or the `Released` of a hold released as that ended.
//!
//! A hold runs one operation at a time: a prepare, a push or a save. A push
//! or a save asked for while another is under way, or of a hold whose
//! prepare did not succeed, is refused. A request that names a hold that has
//! been released is dropped: the name travelled down while the `Released`
//! travelled up (5.2).

use alloc::boxed::Box;

use skein_lib::{Time, Token};

use crate::git::{Commit, Done, Missing, Op};

/// parent -> checkout
#[derive(PartialEq, Eq, Debug)]
#[expect(clippy::large_enum_variant, reason = "bounded diagnostics stay inline and are included in worst_case")]
pub enum Event {
    /// Prepare a workspace for `spec`, on behalf of `client`, which holds it
    /// until it releases it. Answered by exactly one `Prepared`, after a
    /// `Held` if it was admitted.
    Prepare { client: Token, spec: Spec },
    /// Commit what each writable repository of the workspace `hold` holds,
    /// exactly as it is, and push it to the repository's push branch.
    /// Answered by exactly one `Pushed`.
    Push { hold: Token, message: Message },
    /// The same, to the saved-work branch `branch` of each writable
    /// repository. Answered by exactly one `Saved`.
    Save { hold: Token, branch: Box<[u8]>, message: Message },
    /// End the prepare, push or save under way for `hold` once the operation
    /// in flight has settled, without starting another: its answer says it
    /// was aborted, and nothing touches the workspace afterwards. A prepare's
    /// operation is cancelled; a push's or a save's is not, but runs to its
    /// end, which its deadline bounds, so that a push that lands is reported
    /// landed. With nothing under way, nothing happens.
    Abort { hold: Token },
    /// Give the workspace of `hold` back to the cache, aborting what is under
    /// way first, as `Abort` does. Answered by exactly one `Released`, once
    /// nothing touches the workspace.
    Release { hold: Token },
    /// Terminal for `Io`.
    Done { owner: Token, done: Done },
}

/// checkout -> parent
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    /// The prepare for `client` was admitted, and `hold` names its hold from
    /// now on.
    Held { client: Token, hold: Token },
    /// The prepare for `client` has ended: exactly one per `Prepare`.
    Prepared { client: Token, prepared: Prepared },
    /// The push for `client` has ended: exactly one per `Push`.
    Pushed { client: Token, outcome: Outcome },
    /// The save for `client` has ended: exactly one per `Save`.
    Saved { client: Token, outcome: Outcome },
    /// The workspace `client` held is back in the cache: exactly one per
    /// `Release`, once nothing touches the workspace.
    Released { client: Token },
    /// Ask io for `op`, giving up at `deadline`: io runs the race (5.3).
    Io { owner: Token, op: Op, deadline: Time },
    /// Abandon the `Io` in flight for `owner`. Its terminal event still comes:
    /// `Done` with `Cancelled`, or whichever outcome won the race.
    Cancel { owner: Token },
}

/// What a workspace is prepared for.
#[derive(PartialEq, Eq, Debug)]
pub struct Spec {
    /// The workstream: the work on one item, whose runs share a workspace, so
    /// that a later one finds the repositories already cloned. Compared byte
    /// for byte, never interpreted.
    pub key: Box<[u8]>,
    /// The repositories, side by side, each named once.
    pub repositories: Box<[Repository]>,
}

/// A repository of a spec.
#[derive(PartialEq, Eq, Debug)]
pub struct Repository {
    /// Its directory in the workspace, beside the others': one path
    /// component, so neither empty, `.` nor `..`, without `/` or NUL, and not
    /// `.git` in any case.
    pub name: Box<[u8]>,
    /// The forge's address for it, which the protocol layer maps to a URL:
    /// never interpreted.
    pub remote: Box<[u8]>,
    pub start: Start,
    /// Who the worker is to the forge for this repository, and who commits to
    /// it: a name the protocol layer maps to credentials and an author.
    pub identity: u32,
    /// The branch a change is pushed to, if the repository may be written.
    pub push: Option<Box<[u8]>>,
    /// Exact old head required for the push branch; None retains v1's
    /// ordinary fast-forward. Successful pushes advance this condition.
    pub expected: Option<Commit>,
}

/// Where a repository starts. Whatever the workspace held, nothing local is
/// authoritative: it is fetched and checked out afresh.
#[derive(PartialEq, Eq, Debug)]
pub enum Start {
    /// The tip of a base branch. If the forge does not have it, it is created
    /// there from the default branch, only created, never moved, for a
    /// repository that may be written; for one that may not, it is missing.
    Base { branch: Box<[u8]> },
    /// The tip of a branch, which must exist.
    Branch { branch: Box<[u8]> },
    /// A commit, which must exist.
    Commit { commit: Commit },
    /// Saved work: the tip of the saved-work branch, which must exist.
    Saved { branch: Box<[u8]> },
    /// Fetch the branch and base, check out the branch and leave their merge
    /// in progress. The next commit records both parents even if unchanged.
    Merge { branch: Box<[u8]>, base: Commit },
}

/// A commit's message.
#[derive(PartialEq, Eq, Debug)]
pub struct Message {
    pub title: Box<[u8]>,
    pub body: Box<[u8]>,
}

/// How a prepare ended.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Prepared {
    /// The workspace is ready, in the directory io names `workspace`: each
    /// repository at its starting point.
    Ready { workspace: Token, conflicts: Box<[Conflicts]> },
    /// Refused at the entrance: nothing is held.
    Refused { refusal: Refusal },
    /// It failed: the hold stays, for the client to release.
    Failed { failure: Failure },
    /// It was aborted: the hold stays, for the client to release.
    Aborted,
}

/// Originally conflicted paths in one repository, named by its spec index.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Conflicts {
    pub repository: u32,
    pub files: Box<[Box<[u8]>]>,
}

/// Why a prepare failed.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Failure {
    /// The forge could not be reached, something failed on the worker's
    /// side, or an operation ran out of time: trying again may succeed.
    Transient,
    /// The forge does not have what the spec names for the repository at
    /// `repository`, its place in the spec: the repository, a branch (the
    /// default branch, for a base branch to be created from) or a commit.
    Missing { repository: u32, missing: Missing },
    /// The forge refused the identity of the repository at `repository`.
    Refused { repository: u32 },
}

/// Why a request was refused at the entrance.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Refusal {
    /// A prepare: the workstream's workspace is held. A push or a save: the
    /// hold is not ready, as an operation is under way or its prepare did not
    /// succeed.
    Busy,
    /// A prepare: every workspace is held, and the cache has no room for
    /// another.
    Full,
    /// It does not fit the limits.
    Invalid,
}

/// How a push or a save ended.
#[derive(PartialEq, Eq, Debug)]
pub enum Outcome {
    /// What came of each repository, in the spec's order. Pushing several is
    /// not atomic: each lands or not on its own.
    Pushed { landings: Box<[Landing]> },
    /// Refused at the entrance: nothing was committed or pushed.
    Refused { refusal: Refusal },
}

/// What came of one repository in a push or a save. A push that failed in a
/// way that may have landed (it ran out of time, broke, or lost the forge on
/// the way) is checked by fetching its branch: it landed if the branch is at
/// its commit.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
#[expect(clippy::large_enum_variant, reason = "bounded diagnostics stay inline and are included in worst_case")]
pub enum Landing {
    /// Originally conflicted paths still contain a merge marker. No commit
    /// or push was made for this repository.
    Conflicted { files: Box<[Box<[u8]>]> },
    /// A failed commit or push, with bounded diagnostics supplied by io.
    Explained { fault: crate::git::Fault, diagnostic: crate::git::PushDiagnostic },
    /// `commit` is on the branch now.
    Landed { commit: Commit },
    /// The branch is somewhere the commit does not descend from, so the push
    /// was rejected. For a push: the branch moved since the workspace started
    /// from it, or since the last push landed there. For a save: the
    /// saved-work branch moved since the workspace started from it; a save
    /// from any other start to a saved-work branch that exists is always
    /// moved.
    Moved,
    /// The forge could not be reached, something failed on the worker's
    /// side, or it ran out of time, and it did not land: trying again may
    /// succeed.
    Failed,
    /// The forge refused the push.
    Refused,
    /// Nothing to push: the tree is as the repository started (or, for a
    /// push, as the last push left it). Every read-only repository's.
    Unchanged,
    /// Not pushed: an abort or a release came first. A push or a save in
    /// flight is never cancelled: the repository it is pushing when they come
    /// has its own landing, and those after it are aborted.
    Aborted,
}
