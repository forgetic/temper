//! The records that cross the boundary with the session's parent, the
//! top-level model (4.5), which routes them to and from the protocol layer and
//! the session's opener. The session defines them; its parent depends on it.
//!
//! Two shapes cross it. A session's lifecycle, with its opener: an
//! [`Event::Open`] is answered by exactly one [`Request::Ended`], after an
//! [`Request::Opened`] that names the session if it was admitted, and any number
//! of [`Request::Yielded`] in between; the opener addresses the session by that
//! name, and every record back carries the opener's token (4.2). And requests out
//! with exactly one terminal event in (a [`Request::Complete`] is ended by one of
//! [`Event::Completed`], [`Event::Failed`] or [`Event::Cancelled`]). A request's
//! `owner` is the session's token, echoed on its terminal event.

use alloc::boxed::Box;

use temper_lib::{Duration, Token};

use crate::llm::{Completion, Endpoint, Failure, Prompt, Tool, Usage};

/// parent -> session
#[derive(PartialEq, Eq, Debug)]
pub enum Event {
    /// Open a session for `spec`, on behalf of `opener`. Answered by exactly one
    /// `Ended`, after an `Opened` if the session was admitted.
    Open { opener: Token, spec: Spec },
    /// A new user message for a yielded session, which calls the LLM again.
    /// Sent only while the session is yielded; a `session` that has ended
    /// meanwhile is dropped.
    Continue { session: Token, content: Box<[u8]> },
    /// End the session, whatever its state: it cancels what is in flight, and
    /// ends once that has settled. A `session` that has ended is dropped.
    Close { session: Token },
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

/// session -> parent
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    /// The session for `opener` was admitted, and `session` names it from now
    /// on.
    Opened { opener: Token, session: Token },
    /// The LLM stopped calling tools, saying `text` (its message's text blocks,
    /// one after another). The session waits for `Continue` or `Close`.
    Yielded { opener: Token, stop: Yield, text: Box<[u8]> },
    /// The session for `opener` has ended, after `turns` completions that used
    /// `usage`: exactly one per `Open`, once nothing the session asked for is
    /// in flight.
    Ended { opener: Token, end: End, turns: u32, usage: Usage },
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

/// What a session is opened for.
#[derive(PartialEq, Eq, Debug)]
pub struct Spec {
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

/// Why a session yielded: how the LLM stopped calling tools.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Yield {
    /// The LLM finished its turn.
    Done,
    /// The LLM ran out of tokens mid-answer.
    Truncated,
    /// The LLM declined to answer.
    Refused,
    /// The LLM asked for tools and named none.
    Malformed,
}

/// How a session ended.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum End {
    /// Refused at the entrance: every session slot is taken.
    Busy,
    /// Refused at the entrance: the spec does not fit the limits.
    Invalid,
    /// Its opener closed it.
    Closed,
    /// A call failed, for good or after its retries ran out.
    Failed { failure: Failure },
    /// The session used every completion it was allowed.
    TurnLimit,
    /// The conversation outgrew the session's message or byte limit.
    TranscriptFull,
    /// The session ran out of time.
    Expired,
}
