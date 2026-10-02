//! The records that cross the boundary with the run's parent, the top-level
//! model (4.5). The run defines them; its parent depends on it.
//!
//! All three of the run's faces cross here:
//!
//! - The worker's, which the parent routes to and from the protocol layer. A
//!   [`Event::Start`] is a call, answered by exactly one [`Request::Answer`].
//!   An admitted run is named by [`Request::Admitted`] first, so that a
//!   [`Event::Cancel`] can name it. A run's host call, [`Request::Push`], is
//!   ended by exactly one [`Event::Pushed`], or after a
//!   [`Request::CancelHost`] by [`Event::HostCancelled`] if the cancel won.
//! - io's, for what the run itself does in its checkout, which the parent
//!   routes to and from the protocol layer. A [`Request::Read`] is ended by
//!   exactly one [`Event::Read`], a [`Request::Probe`] by one
//!   [`Event::Probed`], and a [`Request::Check`] by one [`Event::Checked`],
//!   or after a [`Request::Abort`] by [`Event::Aborted`] if the abort won.
//!   Each carries its deadline, and io runs the race (5.3).
//! - The conversations', which the parent translates to and from the session
//!   sub-model's vocabulary. A [`Request::Open`] is ended by exactly one
//!   [`Event::Ended`], after a [`Event::Started`] unless the conversation was
//!   refused at its entrance. Every event about a conversation carries the
//!   run's token for it, `conversation`; the run addresses a conversation by
//!   `peer`, the token it gave back when it started (4.2). A conversation's
//!   [`Event::Delegated`] call is ended by exactly one [`Request::Return`],
//!   after an [`Event::Withdraw`] too; the call is named by the
//!   conversation's own token for it, `call`.
//!
//! A request's `owner` is the run's token for what asked: the run itself for
//! a read or a probe, a finishing call for a check, a push or their cancels.
//! It is echoed on the terminal.

use alloc::boxed::Box;

use temper_lib::{ReplyTo, Time, Token};

use crate::budget::{Budget, Exhausted, Spend};
use crate::charter::{Charter, Checkout, Families, Llm, Tools};
use crate::outcome::{Change, Declared, Problems};

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
    /// Terminal for `Read`.
    Read { owner: Token, read: Read },
    /// Terminal for `Probe`: whether an executable file is at the place. A
    /// failure or a deadline passed reads as not.
    Probed { owner: Token, executable: bool },
    /// A call the conversation's LLM made of the run, which the run answers
    /// with one `Return`.
    Delegated { conversation: Token, call: Token, ask: Ask },
    /// The conversation abandons its call `call`: it is closing, or the call's
    /// time ran out. Its `Return` still comes.
    Withdraw { conversation: Token, call: Token },
    /// Terminal for `Check`: what the checks' process did.
    Checked { owner: Token, ran: Ran },
    /// Terminal for `Check`, after `Abort`: the checks were stopped.
    Aborted { owner: Token },
    /// Terminal for `Push`.
    Pushed { owner: Token, push: Push },
    /// Terminal for a host call, after `CancelHost`: it was abandoned.
    HostCancelled { owner: Token },
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
    /// Read the first `max` bytes of the regular file at `at`, following
    /// symbolic links within its root, giving up at `deadline`.
    Read { owner: Token, at: Place, max: u32, deadline: Time },
    /// Find out whether an executable file is at `at`, giving up at
    /// `deadline`.
    Probe { owner: Token, at: Place, deadline: Time },
    /// Run the executable at `program`, in its repository's root, as a
    /// contained process, stopping it at `deadline`; keep the last `tail`
    /// bytes of what it writes.
    Check { owner: Token, program: Place, deadline: Time, tail: u32 },
    /// Stop the `Check` in flight for `owner`. Its terminal still comes:
    /// `Aborted`, or `Checked` if the checks ended first.
    Abort { owner: Token },
    /// To the worker: checks of the run it names `worker` are running until
    /// `deadline` at the latest, so its watchdog waits that long. A notice,
    /// with no terminal. It is a request, not only a fact (`CheckStarted`),
    /// because the watchdog decides on it, and nothing may depend on whether
    /// a fact is kept.
    Checking { worker: Token, deadline: Time },
    /// To the worker, a host call: commit what the checkout of the run it
    /// names `worker` holds, exactly as it is, and push it, with `change`'s
    /// title and body.
    Push { worker: Token, owner: Token, change: Change },
    /// Abandon the host call in flight for `owner`. Its terminal still comes:
    /// `HostCancelled`, or whichever outcome won the race.
    CancelHost { owner: Token },
    /// The one terminal for the conversation's call `call`.
    Return { call: Token, result: Returned },
}

/// What a conversation's LLM asks of the run.
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Ask {
    /// Finish the run with `outcome`.
    Finish { outcome: Declared },
    /// Open a sub-agent: a conversation of its own on `brief`, with families
    /// of tools no wider than the asker's, on the LLM named `llm` among the
    /// charter's (the main conversation's if none is named), and a share of
    /// the budget no larger than `share` (what the run has left, if none is
    /// asked for). Its last message is the call's result.
    SubAgent { brief: Box<[u8]>, families: Families, llm: Option<Box<[u8]>>, share: Option<Spend> },
}

/// The run's answer to a delegated call.
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Returned {
    /// The outcome is accepted: the run finishes with it.
    Accepted,
    /// The outcome does not fit what the run may finish with.
    Rejected { problems: Problems },
    /// The checks of the repository `repository` failed, as `ran` says.
    ChecksFailed { repository: Box<[u8]>, ran: Ran },
    /// The change was not pushed: its branch moved since the run started, and
    /// the run ends.
    Moved,
    /// The change was not pushed: the push failed.
    Unpushed,
    /// Nothing was decided: the call was withdrawn, or the run is ending
    /// otherwise.
    Cancelled,
    /// The run has no room for another call now; it may have later.
    Busy,
    /// The sub-agent's last message: at most the run's limit of its first
    /// bytes, with the `cut` bytes past them dropped, and why it stopped.
    Answered { text: Box<[u8]>, cut: u64, stop: Stop },
    /// The sub-agent ended without an answer: refused at its entrance, its
    /// LLM failed, or its share of the budget ran out.
    Unanswered { end: End },
    /// The run would not open the sub-agent.
    Refused { refusal: AskRefusal },
}

/// Why a run would not open a sub-agent.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum AskRefusal {
    /// The asker may not ask for sub-agents, or asked for families of tools
    /// it does not have itself.
    NotGranted,
    /// The sub-agent would be nested deeper than a run's limit.
    TooDeep,
    /// The run has as many conversations as it may have at once.
    TooMany,
    /// The charter has no LLM of that name for sub-agents.
    UnknownLlm,
    /// The share asked for, or what the run has left, leaves no room for a
    /// turn.
    Unworkable,
    /// The run has spent past its budget.
    Over,
}

/// What a check's process did: how it ended, and the tail of what it wrote,
/// with the `cut` bytes before the tail dropped.
#[derive(PartialEq, Eq, Hash, Debug)]
pub struct Ran {
    pub exit: Exit,
    pub output: Box<[u8]>,
    pub cut: u64,
}

/// How a check's process ended.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Exit {
    /// It exited with `code`; zero passes.
    Code { code: u8 },
    /// A signal killed it.
    Signalled,
    /// Its deadline passed, and io stopped it.
    TimedOut,
    /// It could not be started.
    Unstarted,
}

/// How a push ended.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Push {
    /// The change is pushed.
    Done,
    /// The branch moved since the run started: nothing was pushed.
    Moved,
    /// The push failed.
    Failed,
}

/// Where a file is, for the run's own io: a repository's root, as io names
/// it, and the path beneath it, names joined by `/`. io resolves it beneath
/// the root.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Place {
    pub root: Token,
    pub path: Box<[u8]>,
}

/// What a `Read` found. A file is read as text: io checks that it is UTF-8,
/// so the model never parses, and cuts it at a character boundary, so what
/// the run passes on to an LLM is text too.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Read {
    /// The file's first characters, in at most as many bytes as asked for,
    /// cut where a character ends; `whole` when they are all of it.
    Text { text: Box<[u8]>, whole: bool },
    /// Nothing is there, or not a regular file.
    Missing,
    /// The file is not UTF-8 text.
    NotText,
    /// io failed, or the deadline passed first.
    Failed,
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
    /// Whether its LLM may call `finish`, which the conversation runs as a
    /// write: alone, never beside another call. Only main may.
    pub finish: bool,
    /// The families it has: its tools, and whether its LLM may ask for
    /// sub-agents, a call whose effect follows the families asked for (a
    /// sub-agent that may modify or run commands is a write).
    pub families: Families,
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
    ///
    /// A run is refused at its own entrance, before `Admitted`; or after
    /// `Admitted`, when its main conversation is refused at the
    /// conversations' entrance. The run then did nothing but look in its
    /// checkout: `Busy` says the agent had no room for the conversation, and
    /// a later retry may find some; `Invalid(Conversation)` that its opening
    /// does not fit the conversations' limits, which a retry will not change.
    Refused(Refusal),
    /// The run finished with `outcome`, having spent `spent`. A change has
    /// been pushed.
    Accepted { outcome: Declared, spent: Spend },
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
    /// The LLM's `max_tokens`, or a sub-agent LLM's, is zero or beyond the
    /// limits, or there are more sub-agent LLMs than a run may hold.
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
    /// The branch the change is pushed to moved since the run started: no
    /// change this run makes can land, and the engine plans again from what
    /// the forge holds now.
    Stale,
}

/// The run's rules, as the LLM broke them.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Policy {
    /// It kept stopping without finishing, through `nudges` nudges, having
    /// called `finish` with `rejected` outcomes that were refused.
    Unfinished { nudges: u32, rejected: u32 },
}
