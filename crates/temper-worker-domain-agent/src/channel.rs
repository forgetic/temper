//! The channel between the worker and an agent process (worker-domain.md,
//! section 6; agent-domain.md, section 8): the messages that go down to the
//! agent's run and come up from it, as the protocol layer decodes and encodes
//! them over the process's pipes. The worker reads in them only what it acts
//! on; the rest (a charter, a snapshot, inbound events, a host call's body
//! and its answer, a fact, an outcome) is opaque bytes, passed through.
//!
//! Down, in this order: the start message, first and once; then inbound
//! events, answers to the run's host calls, and at most one cancel, in the
//! order they were given.
//!
//! Up, the channel's rules, which the agent child domain enforces:
//!
//! - Until the run says how it finishes, it may make host calls, withdraw
//!   them, tell facts, report long operations and say that it waits for an
//!   inbound event, in any order. A host call is named by the run, and its
//!   name is in flight from the call until its answer is sent down: a call
//!   that reuses a name in flight breaks the rules, and so does withdrawing a
//!   call twice while the client has yet to answer it. A withdraw that finds
//!   no call waiting for the client crossed its answer on the way, and is
//!   dropped.
//! - A long operation announces at most `Limits::long_span`, and a run that
//!   waits says it has read no more inbound events than were sent down to it.
//! - [`Up::Finish`] (ended, parked or failed) is the run's last word: anything
//!   after it breaks the rules.
//! - Every payload is within the agent child domain's limits, and a message the
//!   protocol layer cannot decode is malformed: either breaks the rules.
//!
//! An agent that breaks the rules has failed, and is stopped.

use alloc::boxed::Box;

use skein_lib::{Duration, Token};

/// agent -> worker
#[derive(PartialEq, Eq, Debug)]
pub enum Up {
    /// A host call, which the run names `call`: answered by exactly one
    /// [`Down::Answer`].
    Call { call: Token, ask: Ask },
    /// The run withdraws its call `call`, its own deadline for it having
    /// passed. The call stays in flight until its answer, which still comes,
    /// once: as withdrawn, or with what came of it.
    Withdraw { call: Token },
    /// A fact of the run, for the engine: passed on as it is, best effort. It
    /// counts as progress.
    Fact { fact: Box<[u8]> },
    /// The run started an operation that may run for up to `span`, such as
    /// the repository's checks: the watchdog waits until then, or until the
    /// run says it is done, and its no-progress clock runs from there.
    Long { span: Duration },
    /// The long operation the run reported is done.
    LongDone,
    /// The run waits for its next inbound event, having read `heard` of those
    /// sent down to it: the watchdog's clock pauses until one is delivered,
    /// if it has read them all.
    Waiting { heard: u64 },
    /// How the run finishes: its last word. Its process exits next.
    Finish { finish: Finish },
}

/// worker -> agent
#[derive(PartialEq, Eq, Debug)]
pub enum Down {
    /// The first message: what the run starts with, passed through. Where the
    /// repositories sit is the protocol layer's to add, from the spawn.
    Start { charter: Box<[u8]>, snapshot: Option<Box<[u8]>> },
    /// An inbound event for the run, passed through.
    Event { event: Box<[u8]> },
    /// The one answer to the run's host call `call`.
    Answer { call: Token, reply: Reply },
    /// The worker cancels the run: it winds down and says how it finishes.
    Cancel,
}

/// What a host call asks.
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Ask {
    /// Commit what the checkout holds, with `message`, and push it. The
    /// worker serves it.
    Push { message: Box<[u8]> },
    /// A forge read or an outlet, relayed to the engine as it is.
    Relay { body: Box<[u8]> },
}

/// The answer to a host call.
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Reply {
    /// The engine's answer to a relayed call, as it is.
    Relayed { answer: Box<[u8]> },
    /// How the push went.
    Pushed(Push),
    /// The run is cancelled or ending: nothing was done, or what was done is
    /// not the run's to hear of.
    Unavailable,
    /// The run has as many calls in flight as it may: nothing was done.
    Busy,
    /// The run withdrew the call: nothing more is done for it.
    Withdrawn,
    /// The answer holds more bytes than may go down: it was dropped.
    TooLarge,
}

/// How a push went, as the run is told.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Push {
    /// Every repository with a change landed it.
    Done,
    /// A branch moved since the run started: no change of the run can land
    /// there.
    Moved,
    /// A push failed, and none moved.
    Failed,
    /// No repository had a change.
    Nothing,
}

/// How the run finishes, as it says.
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Finish {
    /// It ended with `outcome`, its declared outcome, passed through.
    Ended { outcome: Box<[u8]> },
    /// It parked, handing over `snapshot` if it has one.
    Parked { snapshot: Option<Box<[u8]>> },
    /// It failed, for `failure`.
    Failed { failure: RunFailure },
}

/// Why a run failed, as it reports it (agent-domain.md, 4.2).
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
