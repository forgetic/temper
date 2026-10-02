//! The engine's API towards its workers, as the engine's model layer sees it
//! once the protocol layer has framed a message.
//!
//! The engine names a run and each attempt at it with tokens of its own, and
//! every message about a run carries both. What a worker passes through
//! without reading is opaque bytes: the charter, inbound events, relayed
//! calls and their answers, outcomes and snapshots.

use alloc::boxed::Box;

use temper_lib::{Duration, Token};

/// What the engine gives a worker for one run (worker-model.md, 4.1).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Assignment {
    /// The engine's names for the run and for this attempt at it.
    pub run: Token,
    pub attempt: Token,
    pub workspace: Workspace,
    /// The saved-work branch, if unfinished work is saved.
    pub save: Option<Box<[u8]>>,
    /// What the agent's run is given: a [`Charter`], encoded as the `charter`
    /// module says. The worker passes it through.
    pub charter: Box<[u8]>,
    /// The state of a parked run to resume from.
    pub snapshot: Option<Box<[u8]>>,
}

/// The checkout a run works in.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Workspace {
    /// Names the checkout, so later runs of the same work find it cached.
    pub key: Box<[u8]>,
    pub repositories: Box<[Repository]>,
}

/// A repository of a workspace. Names are compared byte for byte.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Repository {
    /// The directory it sits in, beside the others, which is what the agent
    /// calls it: one path component, unique within the workspace.
    pub name: Box<[u8]>,
    /// The forge's address for it.
    pub remote: Box<[u8]>,
    pub start: Start,
    pub access: Access,
}

/// Where a repository's checkout starts.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Start {
    /// A base branch, made from the default branch if it does not exist yet.
    Base {
        branch: Box<[u8]>,
    },
    Branch {
        branch: Box<[u8]>,
    },
    Commit {
        commit: Box<[u8]>,
    },
    /// Saved work, on the saved-work branch `branch`.
    Saved {
        branch: Box<[u8]>,
    },
}

/// Whether a repository may be written.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Access {
    ReadOnly,
    /// A change is pushed to `push`, as the push identity `identity`, a name
    /// the worker's protocol layer maps to credentials.
    Writable {
        push: Box<[u8]>,
        identity: Box<[u8]>,
    },
}

/// What a worker says when it dials in, and again when it reconnects.
#[derive(PartialEq, Eq, Hash, Debug)]
pub struct Hello {
    /// How many runs it hosts at once.
    pub slots: u32,
    /// The workstreams it holds checkouts for.
    pub workstreams: Box<[Box<[u8]>]>,
    /// The runs it hosts: none when it first dials in; on a reconnect, those
    /// it kept while contact was lost.
    pub hosting: Box<[Hosted]>,
}

/// A run a worker hosts.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Hosted {
    pub run: Token,
    pub attempt: Token,
}

/// A worker's answer for an attempt.
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Answer {
    /// Refused at the entrance: nothing was done.
    Refused(Refusal),
    /// The run ended with `outcome`, its declared outcome.
    Ended { outcome: Box<[u8]>, work: Work },
    /// The run parked, with its snapshot if it had one.
    Parked { snapshot: Option<Box<[u8]>>, work: Work },
    /// The run failed, for `failure`.
    Failed { failure: Failure, work: Work },
}

/// What a run left on the forge: the repositories its pushes landed in, by
/// their place in the assignment, ascending; and its save, if one was made,
/// each repository's outcome in the assignment's order.
#[derive(PartialEq, Eq, Hash, Debug)]
pub struct Work {
    pub landed: Box<[u32]>,
    pub saved: Option<Box<[Landing]>>,
}

/// What became of one repository in a save.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Landing {
    /// Its work is on the branch.
    Landed,
    /// The branch moved: nothing was pushed.
    Moved,
    /// The push failed.
    Failed,
    /// It had nothing to save.
    Unchanged,
}

/// Why an assignment was refused.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Refusal {
    /// The worker has no room for it now.
    Busy,
    /// It does not fit the worker's limits.
    Invalid,
}

/// Why a run failed (worker-model.md, 4.3): what the engine acts on.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Failure {
    /// Its workspace could not be prepared.
    Unprepared(Preparation),
    /// The run failed, as it reports it.
    Run(RunFailure),
    /// Its agent failed.
    Agent(AgentFailure),
    /// It was cancelled.
    Cancelled(Cause),
}

/// Why a workspace could not be prepared.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Preparation {
    /// The forge could not be reached.
    Transient,
    /// A repository, branch or commit does not exist.
    Permanent,
}

/// Why a run failed, as it reports it (agent-model.md, 4.2).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum RunFailure {
    /// The LLM could not do the work.
    Model,
    /// The run's budget ran out.
    Budget,
    /// The LLM did not keep to the run's rules.
    Policy,
    /// The run was cancelled.
    Cancelled,
    /// The branch a change is pushed to moved since the run started.
    Stale,
}

/// How a run's agent failed.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum AgentFailure {
    /// It could not be started.
    Unstarted,
    /// It exited without saying how its run finishes.
    Exited,
    /// It broke the channel's rules.
    Rules,
    /// It made no progress.
    NoProgress,
    /// It ran past its wall time.
    WallTime,
}

/// Who cancelled a run.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Cause {
    /// The engine.
    Engine,
    /// The worker, having lost contact with the engine past its grace.
    Contact,
    /// The worker, shutting down.
    Shutdown,
}

/// Why a worker did not pass an inbound event on to its run.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Bounce {
    /// It holds more bytes than the worker's limits allow.
    TooLarge,
    /// The run is not live yet, and holds as many events as it may.
    Full,
    /// The run is ending.
    Ending,
}

/// The engine's answer to a relayed call.
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Reply {
    /// What the call asked for, opaque.
    Answer { body: Box<[u8]> },
    /// The engine could not do what the call asked.
    Error,
}

/// What a run is given (agent-model.md, 4.1), less its repositories, which
/// the worker adds from the workspace, with where it put them.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Charter {
    /// The text the run's LLM works from.
    pub brief: Box<[u8]>,
    pub tools: Tools,
    /// Whether the LLM may read the forge.
    pub forge: bool,
    /// Whether the LLM may ask for sub-agents.
    pub agents: bool,
    /// The outlets the LLM may use, by name.
    pub outlets: Box<[Box<[u8]>]>,
    pub outcome: Outcome,
    pub budget: Budget,
    /// The endpoint the agent reaches the LLM at, and the model it asks for.
    pub endpoint: u32,
    pub model: Box<[u8]>,
    pub max_tokens: u32,
    /// The models sub-agents may run on, at the same endpoint.
    pub models: Box<[Box<[u8]>]>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Tools {
    pub read: bool,
    pub write: bool,
    pub shell: bool,
}

/// What the run may finish with.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Outcome {
    pub change: bool,
    /// Whether a change must pass the checks its repositories have.
    pub checks: bool,
    pub verdicts: Box<[Verdict]>,
}

/// A verdict the run may finish with, and what it must hold.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Verdict {
    pub name: Box<[u8]>,
    pub min_children: u32,
    pub max_children: u32,
    pub kinds: Box<[Box<[u8]>]>,
    pub fields: Box<[Box<[u8]>]>,
}

/// What the run may spend.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Budget {
    pub turns: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub wall_time: Duration,
}
