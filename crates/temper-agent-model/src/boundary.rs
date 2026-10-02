//! The records that cross the boundary with the protocol layer (4.4). The
//! model defines them; the protocol crate depends on the model.
//!
//! For now the variants mirror the session sub-model's, and their payloads are
//! the session's own types. The `route` module translates each variant to the
//! session's vocabulary and back.
//!
//! Two shapes cross it: calls up and replies down (a [`Event::Run`] is answered
//! by exactly one [`Request::Reply`]), and requests down with exactly one
//! terminal event up (a [`Request::Complete`] is ended by one of
//! [`Event::Completed`], [`Event::Failed`] or [`Event::Cancelled`]). A request's
//! `owner` is the session's token, echoed on its terminal event.

use alloc::boxed::Box;

use temper_agent_model_session::llm::{Completion, Failure, Prompt};
use temper_agent_model_session::{Report, Task, ToolCall};
use temper_lib::{Duration, ReplyTo, Token};

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
