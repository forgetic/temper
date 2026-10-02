//! The records that cross the boundary with the run's parent, the top-level
//! model (4.5). The run defines them; its parent depends on it.
//!
//! Both of the run's faces cross here:
//!
//! - The worker's, which the parent routes to and from the protocol layer. A
//!   [`Event::Start`] is a call, answered by exactly one [`Request::Answer`].
//!   An admitted run is named by [`Request::Admitted`] first, so that a
//!   [`Event::Cancel`] can name it.
//! - The conversations', which the parent translates to and from the session
//!   sub-model's vocabulary. A [`Request::Open`] is ended by exactly one
//!   [`Event::Ended`], after a [`Event::Started`] unless the conversation was
//!   refused at its entrance. Every event about a conversation carries the
//!   run's token for it, `conversation`; the run addresses a conversation by
//!   `peer`, the token it gave back when it started (4.2).

use alloc::boxed::Box;

use temper_lib::{ReplyTo, Token};

use crate::budget::{Budget, Exhausted, Spend};
use crate::charter::{Charter, Checkout, Llm, Tools};

/// parent -> run
#[derive(PartialEq, Eq, Debug)]
pub enum Event {
    /// From the worker, a call: start a run on `charter`, and answer once it
    /// has ended. `worker` is the worker's name for the run, echoed on
    /// `Admitted`.
    Start { reply_to: ReplyTo, worker: Token, charter: Charter },
    /// From the worker: end the run `run` as cancelled. A run that has already
    /// answered, or decided how it ends, ignores it.
    Cancel { run: Token },
    /// The conversation was admitted, and `peer` names it from now on.
    Started { conversation: Token, peer: Token },
    /// The LLM stopped calling tools, `text` being its last message. The
    /// conversation waits for `Say` or `Close`, and its time keeps running.
    Yielded { conversation: Token, stop: Stop, text: Box<[u8]> },
    /// The conversation's LLM completed a turn, spending `spend`.
    Used { conversation: Token, spend: Spend },
    /// Terminal for `Open`: the conversation ended, having spent `spend` in
    /// all, once nothing it started was in flight.
    Ended { conversation: Token, end: End, spend: Spend },
}

/// run -> parent
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    /// To the worker: the run it names `worker` was admitted, and is `run` to
    /// the run sub-model from now on.
    Admitted { worker: Token, run: Token },
    /// To the worker, the answer to a `Start`: exactly one per start.
    Answer { to: ReplyTo, answer: Answer },
    /// Open a conversation, which every event about it names `conversation`.
    Open { conversation: Token, opening: Opening },
    /// A new user message for `peer`, a conversation that has yielded. One that
    /// has ended meanwhile drops it.
    Say { peer: Token, text: Box<[u8]> },
    /// Close `peer`, in any state: it stops what is in flight, then ends.
    Close { peer: Token },
}

/// What a conversation is opened with.
#[derive(PartialEq, Eq, Hash, Debug)]
pub struct Opening {
    /// The LLM it talks to.
    pub llm: Llm,
    pub system: Box<[u8]>,
    /// The first user message.
    pub prompt: Box<[u8]>,
    /// The tools the LLM may run on the checkout.
    pub tools: Tools,
    /// The checkout they act on, and which of it may be written.
    pub checkout: Checkout,
    /// Its share of the run's budget: what the run has left when it opens, and
    /// the time to the run's deadline. The conversation keeps to it.
    pub budget: Budget,
}

/// Why a conversation's LLM stopped calling tools.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Stop {
    /// It finished its turn.
    EndTurn,
    /// It ran out of tokens mid-answer.
    MaxTokens,
    /// It declined to answer.
    Refusal,
    /// It asked for tools and named none.
    NoCalls,
}

/// How a conversation ended.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum End {
    /// The run closed it.
    Closed,
    /// Refused at its entrance: no room for another conversation.
    Busy,
    /// Refused at its entrance: the opening does not fit the conversations'
    /// limits.
    Invalid,
    /// Its LLM could not go on.
    Fault(Fault),
    /// Its share of the budget ran out.
    Budget(Exhausted),
}

/// What kept an LLM from going on.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Fault {
    /// Its provider failed for good: unreachable, overloaded past the
    /// retries, or refusing the call or its credentials.
    Provider,
    /// The conversation outgrew the model's context, or the bytes a
    /// conversation may hold.
    ContextFull,
    /// It kept declining to answer.
    Refused,
    /// It kept running out of tokens mid-answer.
    Truncated,
    /// It kept asking for tools and naming none.
    Malformed,
}

/// The answer to a `Start`.
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Answer {
    /// Refused at the entrance: nothing was done.
    Refused(Refusal),
    /// The run ended without an outcome, having spent `spent`.
    Failed { failure: Failure, spent: Spend },
}

/// Why a run was refused.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Refusal {
    /// No room for another run, or for its conversation.
    Busy,
    /// The charter does not fit the limits.
    Invalid(Invalid),
}

/// What about a charter does not fit the limits.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Invalid {
    /// It holds more bytes than a run may.
    TooLarge,
    /// The checkout lists more repositories than a run may hold, or one name
    /// twice.
    Checkout,
    /// The grants list more outlets than a run may hold, or one name twice.
    Grants,
    /// The outcome spec allows no outcome, lists more verdicts than a run may
    /// hold or one name twice, or has a contract no verdict can meet.
    Outcome,
    /// The budget asks for more than the limits allow, or for no turns, input,
    /// output or time.
    Budget,
    /// The LLM's `max_tokens` is zero or beyond the limits.
    Llm,
    /// The main conversation was refused: its opening does not fit the
    /// conversations' limits.
    Conversation,
}

/// Why a run ended without an outcome: what the worker acts on.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Failure {
    /// The LLM could not do the work.
    Model(Fault),
    /// The run's budget ran out.
    Budget(Exhausted),
    /// The LLM did not keep to the run's rules.
    Policy(Policy),
    /// The worker cancelled the run.
    Cancelled,
}

/// The run's rules, as the LLM broke them.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Policy {
    /// It kept stopping without finishing, through `nudges` nudges.
    Unfinished { nudges: u32 },
}
