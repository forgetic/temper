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
//! [`Event::Failed`] or [`Event::Cancelled`]; a [`Request::Io`], the file and
//! process operations of the session's tools, by [`Event::Done`]). A request's
//! `owner` is the token of whoever asked, echoed on its terminal event.

use alloc::boxed::Box;

use temper_agent_model_session::llm::{Answer, Completion, Failure, Prompt, Usage};
use temper_agent_model_session::{End, Spec, Yield};
use temper_agent_model_tools as tools;
use temper_lib::{Duration, Time, Token};

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
    /// Terminal for `Io`, for the session's tools.
    Done { owner: Token, done: tools::Done },
    /// Terminal for `Delegate`: the opener's answer.
    Answered { owner: Token, answer: Answer },
    /// Terminal for `Delegate`, after `Withdraw`: the call was abandoned.
    AnswerCancelled { owner: Token },
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
    /// Ask io for `op` for the session's tools, giving up at `deadline`: io runs
    /// the race (5.3).
    Io { owner: Token, op: tools::Op, deadline: Time },
    /// Abandon the `Io` in flight for `owner`. Its terminal event still comes:
    /// `Done` with `Cancelled`, or whichever outcome won the race.
    CancelIo { owner: Token },
    /// Ask the opener to serve the delegated call `call`, by `deadline`.
    Delegate { owner: Token, opener: Token, call: Token, deadline: Time },
    /// Abandon the `Delegate` in flight for `owner`. Its terminal event still
    /// comes: `AnswerCancelled`, or `Answered` if the answer won the race.
    Withdraw { owner: Token },
}
