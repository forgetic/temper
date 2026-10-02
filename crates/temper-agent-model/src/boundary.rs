//! The records that cross the boundary with the protocol layer (4.4). The
//! model defines them; the protocol crate depends on the model.
//!
//! For now the variants mirror the session sub-model's, and their payloads are
//! the session's own types: until the run sub-model opens sessions, the
//! protocol layer plays their opener. The `route` module translates each
//! variant to the session's vocabulary and back.
//!
//! Two shapes cross it: a session's lifecycle (an [`Event::Open`] is answered
//! by exactly one [`Request::Ended`], after an [`Request::Opened`] that names
//! the session if it was admitted, and any number of [`Request::Yielded`] and
//! [`Request::Used`] in between), and requests down with exactly one terminal
//! event up (a [`Request::Complete`] is ended by one of [`Event::Completed`],
//! [`Event::Failed`] or [`Event::Cancelled`]). A request's `owner` is the
//! session's token, echoed on its terminal event.

use alloc::boxed::Box;

use temper_agent_model_session::llm::{Completion, Failure, Prompt, Usage};
use temper_agent_model_session::{End, Spec, Yield};
use temper_agent_model_tools as tools;
use temper_lib::{Duration, Token};

/// protocol -> model
#[derive(PartialEq, Eq, Debug)]
pub enum Event {
    /// Open a session for `spec`, on behalf of `opener`. Answered by exactly one
    /// `Ended`, after an `Opened` if the session was admitted.
    Open { opener: Token, spec: Spec },
    /// A new user message for a yielded session. A `session` that has ended
    /// meanwhile is dropped.
    Continue { session: Token, content: Box<[u8]> },
    /// End the session, whatever its state. A `session` that has ended is
    /// dropped.
    Close { session: Token },
    /// Terminal for `Complete`: the LLM produced its next message.
    Completed { owner: Token, completion: Completion },
    /// Terminal for `Complete`: the call produced no message.
    Failed { owner: Token, failure: Failure },
    /// Terminal for `Complete`, after `Cancel`: the call was abandoned.
    Cancelled { owner: Token },
    /// Terminal for `Tool`: what came of the call, a success or a failure.
    ToolDone { owner: Token, outcome: tools::Outcome },
    /// Terminal for `Tool`, after `CancelTool`: the run was abandoned.
    ToolCancelled { owner: Token },
}

/// model -> protocol
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    /// The session for `opener` was admitted, and `session` names it from now
    /// on.
    Opened { opener: Token, session: Token },
    /// The LLM stopped calling tools, saying `text`. The session waits for
    /// `Continue` or `Close`.
    Yielded { opener: Token, stop: Yield, text: Box<[u8]> },
    /// A completion came back: one turn, and `usage` as the provider counts
    /// it.
    Used { opener: Token, usage: Usage },
    /// The session for `opener` has ended: exactly one per `Open`.
    Ended { opener: Token, end: End, turns: u32, usage: Usage },
    /// Ask an LLM for the next assistant message, giving up after `timeout`.
    Complete { owner: Token, prompt: Prompt, timeout: Duration },
    /// Abandon the `Complete` in flight for `owner`. Its terminal event still
    /// comes: `Cancelled`, or whichever outcome won the race.
    Cancel { owner: Token },
    /// Run a call of the tools the session owns.
    Tool { owner: Token, call: tools::Call },
    /// Abandon the `Tool` in flight for `owner`. Its terminal event still
    /// comes: `ToolCancelled`, or `ToolDone` if the run won the race.
    CancelTool { owner: Token },
}
