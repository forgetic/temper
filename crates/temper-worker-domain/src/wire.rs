//! Temper worker wire and checkout vocabulary.

use alloc::boxed::Box;
use skein_lib::Token;
pub use jig_worker_host::{AgentFailure, Bounce, Finish, FinishV2, Grant, Hosting, Phase, Reason, RunFailure, Turn};
pub use crate::push::{PushDiagnostic, PushFailure, PushReason};

/// What the engine gives the worker for one run (worker-domain.md, 4.1).
#[derive(PartialEq, Eq, Hash, Debug)]
pub struct Assignment {
    /// The engine's names for the run and for this attempt at it.
    pub run: Token,
    pub attempt: Token,
    pub workspace: Workspace,
    /// The saved-work branch, if unfinished work is saved.
    pub save: Option<Box<[u8]>>,
    /// What the agent's run is given, passed through.
    pub charter: Box<[u8]>,
    /// The state of a parked run to resume from, passed through.
    pub snapshot: Option<Box<[u8]>>,
    pub grants: Box<[Grant]>,
}

/// An explicit second-version assignment. A snapshot is invalid here.
#[derive(PartialEq, Eq, Hash, Debug)]
pub struct AssignmentV2 {
    pub assignment: Assignment,
    pub transcript: Option<Box<[u8]>>,
}

#[derive(PartialEq, Eq, Hash, Debug)]
pub struct AnswerV2 {
    pub turns: u32,
    pub spent: u64,
    pub ending: EndingV2,
}

#[derive(PartialEq, Eq, Hash, Debug)]
pub enum EndingV2 {
    Refused(Refusal),
    Ended { outcome: Box<[u8]>, work: Work },
    Parked { work: Work },
    Failed { failure: Failure, detail: Box<[u8]>, work: Work },
}

/// The checkout a run works in. The host checks its bounds and passes it on.
#[derive(PartialEq, Eq, Hash, Debug)]
pub struct Workspace {
    /// Names the checkout, so later runs of the same work find it cached.
    pub key: Box<[u8]>,
    pub repositories: Box<[Repository]>,
}

/// A repository of a workspace. Names are compared byte for byte.
#[derive(PartialEq, Eq, Hash, Debug)]
pub struct Repository {
    /// Deployment repository name, echoed in landed work.
    pub tag: u32,
    /// The directory it sits in, side by side with the workspace's others
    /// (worker-domain.md, section 5), and what the agent calls it: one path
    /// component, unique within the workspace.
    pub name: Box<[u8]>,
    /// The forge's address of the repository, never interpreted: the
    /// protocol layer maps it to a URL.
    pub remote: Box<[u8]>,
    pub start: Start,
    pub access: Access,
    /// Who the worker is to the forge for this repository, to read it and to
    /// push to it, and who commits to it: an account the protocol layer maps to
    /// credentials and an author.
    pub identity: u32,
}

/// Where a repository's checkout starts.
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Start {
    Merge {
        branch: Box<[u8]>,
        base: [u8; 32],
    },
    /// A base branch, made from the default branch if it does not exist yet.
    Base {
        branch: Box<[u8]>,
    },
    Branch {
        branch: Box<[u8]>,
    },
    /// A commit, by its object id: the full id in 32 bytes, as the protocol
    /// layer makes it of git's (a SHA-256 id as it is, a SHA-1 id followed by
    /// twelve zero bytes), compared and never looked inside.
    Commit {
        commit: [u8; 32],
    },
    /// Saved work, on the saved-work branch `branch`.
    Saved {
        branch: Box<[u8]>,
    },
}

/// Whether a repository may be written.
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Access {
    /// Version-two writable branch, fenced to the expected previous head.
    WritableV2 {
        push: Box<[u8]>,
        expected: Option<[u8; 32]>,
    },
    ReadOnly,
    /// A change is pushed to `push`.
    Writable {
        push: Box<[u8]>,
    },
}

/// A host call of a run.
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Ask {
    PushV2 {
        title: Box<[u8]>,
        body: Box<[u8]>,
    },
    /// Commit what the checkout holds, with `message`, and push it. The host
    /// serves it.
    Push {
        message: Box<[u8]>,
    },
    /// A forge read or an outlet, relayed to the engine as it is.
    Relay {
        body: Box<[u8]>,
    },
}

/// The answer to a host call.
#[derive(PartialEq, Eq, Hash, Debug)]
#[expect(clippy::large_enum_variant, reason = "bounded diagnostics stay inline and are included in worst_case")]
pub enum Reply {
    /// The engine's answer to a relayed call, as it is.
    Relayed { answer: Box<[u8]> },
    /// How the push went.
    Pushed(Push),
    /// The run is cancelled or ending: nothing was done. A push in flight as
    /// the run leaves live is waited for, and answered with how it went.
    Unavailable,
    /// The run withdrew the relayed call: nothing more is done for it, and
    /// the engine's answer, if one comes, is dropped.
    Withdrawn,
    /// The run has as many calls in flight as it may, or a push in flight
    /// already: nothing was done. Calls answered within the loop's current
    /// iteration keep their slots until its reclaim point, so a busy call may
    /// find room in the next.
    Busy,
}

/// How a push went, as the run is told: done only if every repository with a
/// change landed it. A push the forge refused failed, as the run sees it.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
#[expect(clippy::large_enum_variant, reason = "bounded diagnostics stay inline and are included in worst_case")]
pub enum Push {
    Conflicted {
        repository: u32,
        files: Box<[Box<[u8]>]>,
    },
    Done,
    /// A branch moved since the run started: no change of the run can land
    /// there.
    Moved,
    /// A push failed, and none moved.
    Failed {
        failure: PushFailure,
    },
    /// No repository had a change.
    Nothing,
}

/// What became of one repository in a push or a save.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
#[expect(clippy::large_enum_variant, reason = "bounded diagnostics stay inline and are included in worst_case")]
pub enum Landing {
    Conflicted {
        files: Box<[Box<[u8]>]>,
    },
    /// A failed invocation, with its typed reason and bounded diagnostics.
    Explained {
        failure: PushFailure,
    },
    /// Its change is on the branch, as `commit`, by its object id (as
    /// [`Start::Commit`]'s).
    Landed {
        commit: [u8; 32],
    },
    /// The branch moved since the run started: nothing was pushed.
    Moved,
    /// The push failed, and did not land: a retry may succeed.
    Failed,
    /// The forge refused the push: the identity's permissions, a protected
    /// branch, a hook. A retry will not do better.
    Refused,
    /// It had no change.
    Unchanged,
}

/// The answer to an `Assign`.
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Answer {
    /// Refused at the entrance: nothing was done.
    Refused(Refusal),
    /// The run ended with `outcome`.
    Ended { outcome: Box<[u8]>, work: Work },
    /// The run parked, with its snapshot if it had one.
    Parked { snapshot: Option<Box<[u8]>>, work: Work },
    /// The run failed, for `failure`; `detail` is for operators, never for an
    /// LLM.
    Failed { failure: Failure, detail: Box<[u8]>, work: Work },
}

/// What a run left on the forge: the repositories its pushes landed in, by
/// their deployment tags, ascending, each with the last commit landed
/// there (the head a pull request is to show); and its save, if one was made,
/// each repository's outcome in the assignment's order.
#[derive(PartialEq, Eq, Hash, Debug)]
pub struct Work {
    pub landed: Box<[Landed]>,
    pub saved: Option<Box<[Landing]>>,
}

/// A repository a run's pushes landed in: its deployment tag, and
/// the last commit landed there.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Landed {
    pub tag: u32,
    pub commit: [u8; 32],
}

/// Why an assignment was refused.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Refusal {
    /// Every slot is taken (by a run hosted, or one whose answer the engine
    /// has yet to acknowledge), the run is hosted already under another
    /// attempt that has not answered yet, or the worker is shutting down. A
    /// later retry, on this worker or another, may find room.
    Busy,
    /// The assignment does not fit the limits.
    Invalid(Invalid),
}

/// What about an assignment does not fit the limits.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Invalid {
    /// The workspace lists no repository, or more than a run may hold.
    Repositories,
    /// The workspace lists one repository name twice.
    Duplicate,
    /// A workstream key, repository name, remote, branch is empty
    /// or longer than a name may be; or a repository name is not one
    /// safe path component: `.`, `..`, `.git` in any case, or holding `/` or
    /// NUL.
    Name,
    /// The charter holds more bytes than a run may.
    Charter,
    /// The snapshot holds more bytes than a run may.
    Snapshot,
    Transcript,
    Version,
    Grants,
}

/// Why a hosted run failed (worker-domain.md, 4.3): what the engine acts on.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Failure {
    /// Its workspace could not be prepared.
    Unprepared(Preparation),
    /// The run failed, as it reports it.
    Run(RunFailure),
    /// Its agent failed.
    Agent(AgentFailure),
    /// It was cancelled.
    Cancelled(Reason),
}

/// Why a workspace could not be prepared.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Preparation {
    /// The forge could not be reached, something failed on the worker's
    /// side, an operation ran out of time, or the workspace is held by
    /// another run of its workstream: a retry may work.
    Transient,
    /// The forge does not have what the assignment names for the repository
    /// at `repository`, its place in the assignment: permanent until someone
    /// makes it.
    Missing { repository: u32, missing: Missing },
    /// The forge refused the identity of the repository at `repository`:
    /// permanent until the identity's credentials or permissions change.
    Refused { repository: u32 },
}

/// What the forge does not have.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Missing {
    Repository,
    /// A branch to start from, or the default branch a base branch is to be
    /// made from.
    Branch,
    Commit,
}

