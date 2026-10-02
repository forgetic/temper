//! The agent's API, as the worker's model layer sees it once the protocol
//! layer has framed a message.

use alloc::boxed::Box;

use temper_lib::Duration;

/// What the worker starts a run with.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Charter {
    /// The text the run's LLM works from.
    pub brief: Box<[u8]>,
    pub repositories: Box<[Repository]>,
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

/// A repository the worker checked out for the run.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Repository {
    pub name: Box<[u8]>,
    pub path: Box<[u8]>,
    pub writable: bool,
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

/// A change a run asks the worker to push.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Change {
    pub title: Box<[u8]>,
    pub body: Box<[u8]>,
}

/// How a push went.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Pushed {
    Done,
    /// The branch moved since the run started: nothing was pushed.
    Moved,
    Failed,
}

/// The agent's answer for a run.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Answer {
    /// The agent had no room for it.
    Busy,
    /// It finished, after spending `usage`; a change was pushed.
    Done { usage: Usage },
    /// The agent would not take its charter.
    Invalid,
    /// It ended without an outcome, after spending `usage`.
    Failed { reason: Reason, usage: Usage },
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Reason {
    /// The LLM could not do the work.
    Model,
    /// The run's budget ran out.
    Budget,
    /// The LLM stopped without finishing, however often it was told to go on.
    Unfinished,
    /// The worker cancelled it.
    Cancelled,
    /// The branch moved since the run started: its change cannot land.
    Stale,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Usage {
    pub turns: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}
