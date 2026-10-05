//! The records that cross the boundary with the session's parent, the
//! root domain (4.5), which routes them to and from the protocol layer and
//! the session's opener. The session defines them; its parent depends on it.
//!
//! Two shapes cross it. A session's lifecycle, with its opener: an
//! [`Event::Open`] is answered by exactly one [`Request::Ended`], after an
//! [`Request::Opened`] that names the session if it was admitted, and any
//! number of [`Request::Yielded`] and [`Request::Used`] in between; the opener
//! addresses the session by that name, and every record back carries the
//! opener's token (4.2). And requests out with exactly one terminal event in
//! (a [`Request::Complete`] is ended by one of [`Event::Completed`],
//! [`Event::Failed`] or [`Event::Cancelled`]; a [`Request::Delegate`], to the
//! opener, by [`Event::Answered`] or [`Event::AnswerCancelled`]; a
//! [`Request::Io`], the tools' file and process operations passed on as they
//! are, by [`Event::Done`]). A request's `owner` is echoed on its terminal
//! event: the session's token for a call to the LLM, a tool run's own for a
//! delegated call, and the tools' own for an operation.

use alloc::boxed::Box;

use skein_lib::{Duration, Time, Token};
use temper_legacy_agent_domain_tools as tools;

use crate::llm::{Answer, Completion, Descriptor, Endpoint, Failure, Prompt, Usage};

/// parent -> session
#[derive(PartialEq, Eq, Debug)]
pub enum Event {
    /// Open a session for `spec`, on behalf of `opener`. Answered by exactly one
    /// `Ended`, after an `Opened` if the session was admitted.
    Open { opener: Token, spec: Spec },
    /// Explicit version-two admission, including a fresh session with no history.
    OpenV2 { opener: Token, spec: crate::record::Opening },
    /// A concrete delegated answer, including the spend of a sub-agent served
    /// by this call. The opener charges each child once, at its terminal answer.
    AnsweredV2 { owner: Token, text: Box<[u8]>, error: bool, spent: u64 },
    /// A withdrawn child terminal still reports what it spent before stopping.
    AnswerCancelledV2 { owner: Token, spent: u64 },
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
    /// Terminal for `Io`, for the tools.
    Done { owner: Token, done: tools::Done },
    /// Terminal for `Delegate`: the opener's answer, a success or a failure.
    Answered { owner: Token, answer: Answer },
    /// Terminal for `Delegate`, after `Withdraw`: the call was abandoned.
    AnswerCancelled { owner: Token },
}

/// session -> parent
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    /// The session for `opener` was admitted, and `session` names it from now
    /// on.
    Opened { opener: Token, session: Token },
    /// A settled turn, copied at emission. Its values contain no session tickets.
    Turn { opener: Token, turn: crate::record::Turn },
    /// Version two's cumulative deployment-unit spend, including child calls.
    Priced { opener: Token, spent: u64, overflow: bool },
    /// The LLM stopped calling tools, saying `text` (its message's text blocks,
    /// one after another). The session waits for `Continue` or `Close`, and
    /// its time budget keeps running.
    Yielded { opener: Token, stop: Yield, text: Box<[u8]> },
    /// A completion came back: one turn, and `usage` as the provider counts
    /// it. One per completion, the ones that win a race with a cancel
    /// included.
    Used { opener: Token, usage: Usage },
    /// The session for `opener` has ended, after `turns` completions that used
    /// `usage` (what its `Used` add up to): exactly one per `Open`, once
    /// nothing the session asked for is in flight.
    Ended { opener: Token, end: End, turns: u32, usage: Usage },
    /// Ask an LLM for the next assistant message, giving up after `timeout`.
    Complete { owner: Token, prompt: Prompt, timeout: Duration },
    /// Abandon the `Complete` in flight for `owner`. Its terminal event still
    /// comes: `Cancelled`, or whichever outcome won the race.
    Cancel { owner: Token },
    /// Ask io for `op` for the tools, giving up at `deadline`: io runs the
    /// race (5.3).
    Io { owner: Token, op: tools::Op, deadline: Time },
    /// Abandon the `Io` in flight for `owner`. Its terminal event still comes:
    /// `Done` with `Cancelled`, or whichever outcome won the race.
    CancelIo { owner: Token },
    /// Ask the opener to serve the delegated call `call`, a ticket, and to
    /// answer it by `deadline`, when the session's time runs out: the opener
    /// runs the race, within limits of its own if it has shorter ones, and
    /// answers a call that loses as a failure.
    Delegate { owner: Token, opener: Token, call: Token, deadline: Time },
    /// Abandon the `Delegate` in flight for `owner`, as the session closes.
    /// Its terminal event still comes: `AnswerCancelled`, or `Answered` if
    /// the answer won the race.
    Withdraw { owner: Token },
}

/// What a session is opened for.
#[derive(PartialEq, Eq, Debug)]
pub struct Spec {
    pub endpoint: Endpoint,
    /// The provider's name for the model.
    pub model: Box<[u8]>,
    pub system: Box<[u8]>,
    /// What the session's own tools may do and where (the families the LLM
    /// may call among them, the checkout's repositories, what commands run
    /// with), copied into its kit; and the tools its opener serves.
    pub authority: tools::Authority,
    pub delegated: Box<[Descriptor]>,
    /// The first user message.
    pub prompt: Box<[u8]>,
    /// The most tokens each answer may take, and fewer once the output budget
    /// has less left.
    pub max_tokens: u32,
    pub budget: Budget,
}

/// What a session may spend, from the moment it opens. Every dimension is
/// within the session's `Limits`, or the spec is refused.
///
/// A session starts a completion only while it has turns, input and output
/// tokens left (what it spent is below the budget), its cache reads and writes
/// are within their budget, and its `time` has not run out; when it needs one
/// it may not start, it ends. A completion's tokens are known only once it
/// comes back, so it may take input and cache tokens past their budget: its
/// turn still runs the tools it asked for, and the session ends where it
/// would have started the next. The output budget it cannot pass, as the
/// answer's `max_tokens` is cut to what is left. A zero cache budget
/// therefore ends a session only once a completion touches the cache. Time
/// does not wait for the turn: when it runs out, the session closes at once.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Budget {
    pub turns: u32,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub time: Duration,
}

/// A dimension of a [`Budget`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Dimension {
    Turns,
    Input,
    Output,
    CacheRead,
    CacheWrite,
    Time,
    Unit,
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
    /// The session's budget ran out in the `spent` dimension: it needed a
    /// completion the budget does not leave room for, or its time is up.
    Budget { spent: Dimension },
    /// The conversation outgrew the session's message or byte limit.
    TranscriptFull,
    /// A transcript cannot be resumed. The next attempt must start fresh;
    /// this is a transient run failure (domain/engine.md, 7.2).
    TranscriptRefused { reason: crate::record::Refusal },
    /// A checked price or cumulative spend did not fit the deployment counter.
    PriceOverflow,
}
