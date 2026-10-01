//! The records that cross the boundary with the protocol layer (4.4). The
//! model defines them; the protocol crate depends on the model.
//!
//! Two shapes cross it: calls up and replies down (a [`Event::Run`] is answered
//! by exactly one [`Request::Reply`]), and requests down with exactly one
//! terminal event up (a [`Request::Complete`] is ended by one of
//! [`Event::Completed`], [`Event::Failed`] or [`Event::Cancelled`]). A request's
//! `owner` is the session's token, echoed on its terminal event.

use alloc::boxed::Box;

use temper_lib::{Duration, ReplyTo, Token};

use crate::llm::{Block, Completion, Endpoint, Failure, Prompt, Tool, Usage};

/// protocol -> model
#[derive(PartialEq, Eq, Debug)]
pub enum Event {
    /// A call: run a session for `task`, and answer once it has ended.
    Run { reply_to: ReplyTo, task: Task },
    /// Terminal for `Complete`: the LLM produced its next message.
    Completed { owner: Token, completion: Completion },
    /// Terminal for `Complete`: the call produced no message.
    Failed { owner: Token, failure: Failure },
    /// Terminal for `Complete`, after `Cancel`: the call was abandoned.
    Cancelled { owner: Token },
    /// Terminal for `Tool`: the tool ran. `error` marks a failed run, and
    /// `output` then says why.
    ToolDone { owner: Token, output: Box<[u8]>, error: bool },
    /// Terminal for `Tool`, after `CancelTool`: the run was abandoned.
    ToolCancelled { owner: Token },
}

/// model -> protocol
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    /// The answer to a `Run`: exactly one per run.
    Reply { to: ReplyTo, report: Report },
    /// Ask an LLM for the next assistant message, giving up after `timeout`.
    Complete { owner: Token, prompt: Prompt, timeout: Duration },
    /// Abandon the `Complete` in flight for `owner`. Its terminal event still
    /// comes: `Cancelled`, or whichever outcome won the race.
    Cancel { owner: Token },
    /// Run a tool.
    Tool { owner: Token, call: ToolCall },
    /// Abandon the `Tool` in flight for `owner`. Its terminal event still
    /// comes: `ToolCancelled`, or `ToolDone` if the run won the race.
    CancelTool { owner: Token },
}

/// What a session is asked to do.
#[derive(PartialEq, Eq, Debug)]
pub struct Task {
    pub endpoint: Endpoint,
    /// The provider's name for the model.
    pub model: Box<[u8]>,
    pub system: Box<[u8]>,
    /// The tools the LLM may call.
    pub tools: Box<[Tool]>,
    /// The first user message.
    pub prompt: Box<[u8]>,
    /// The most tokens each answer may take.
    pub max_tokens: u32,
}

/// A tool to run, as the LLM asked for it. `input` is a JSON object.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ToolCall {
    pub name: Box<[u8]>,
    pub input: Box<[u8]>,
}

/// The answer to a `Run`.
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Report {
    /// Refused at the entrance: every session slot is taken.
    Busy,
    /// Refused at the entrance: the task does not fit the limits.
    Invalid,
    /// The session ran and ended, after `turns` completions that used `usage`.
    Ended { outcome: Outcome, turns: u32, usage: Usage },
}

/// How a session ended.
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Outcome {
    /// The LLM finished its turn, with this message.
    Done { content: Box<[Block]> },
    /// A call failed, for good or after its retries ran out.
    Failed { failure: Failure },
    /// The LLM ran out of tokens mid-answer.
    Truncated,
    /// The LLM declined to answer.
    Refused,
    /// The LLM asked for tools and named none.
    Malformed,
    /// The session used every completion it was allowed.
    TurnLimit,
    /// The conversation outgrew the session's message or byte limit.
    TranscriptFull,
    /// The session ran out of time.
    Expired,
}
