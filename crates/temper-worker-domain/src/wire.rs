//! Temper worker wire and checkout vocabulary.

pub use crate::push::{PushDiagnostic, PushFailure, PushReason};
use alloc::boxed::Box;
pub use jig_host::{AgentFailure, AnsweredCall, Bounce, Finish, Grant, Hosting, Phase, Reason, RunFailure, Turn};
use skein_lib::Token;

/// What the engine gives the worker for one run (worker-domain.md, 4.1).
#[derive(PartialEq, Eq, Hash, Debug)]
pub struct RunAssignment {
    /// The engine's names for the run and for this attempt at it.
    pub run: Token,
    pub attempt: Token,
    pub workspace: Workspace,
    /// The saved-work branch, if unfinished work is saved.
    pub save: Option<Box<[u8]>>,
    /// What the agent's run is given, passed through.
    pub charter: Box<[u8]>,
    pub grants: Box<[Grant]>,
}

/// The agent's ordered committed conversation state for a new activation.
#[derive(PartialEq, Eq, Hash, Debug)]
pub struct Assignment {
    pub assignment: RunAssignment,
    pub turns: Box<[Box<[u8]>]>,
    pub answered: Box<[AnsweredCall]>,
}

#[derive(PartialEq, Eq, Hash, Debug)]
pub struct Answer {
    pub turns: u32,
    pub spent: u64,
    pub ending: Ending,
}

#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Ending {
    Refused(Refusal),
    Ended { outcome: Box<[u8]>, work: Work },
    Parked { work: Work },
    Failed { failure: Failure, detail: Box<[u8]>, work: Work },
}

/// The checkout a run works in. An empty repository list means the run has no
/// workspace items, so no checkout is prepared or held.
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
}

/// A host call of a run.
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
    /// The workspace lists more repositories than a run may hold.
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
    Transcript,
    /// A resumed delivery's Smith channel evidence is malformed or disagrees with its outcome.
    DeliveryEvidence,
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
