//! The records that cross the boundary with the agent sub-model's parent, the
//! worker's top-level model (4.5). The agent sub-model defines them; its
//! parent depends on it.
//!
//! Two faces cross it, both through the parent:
//!
//! - The client's: whoever wants a run hosted in an agent process, which the
//!   parent translates to and from the host sub-model's vocabulary. An
//!   [`Event::Spawn`] is ended by exactly one [`Request::Gone`], once the
//!   agent's process has exited and its tree is empty, or at once if it was
//!   refused or could not be spawned; before it, [`Request::Started`] names
//!   the agent if it was spawned, and a client that wants a spawning agent
//!   stopped waits for one or the other. In between, the run's host calls
//!   come up ([`Request::Called`]), each answered by the client with exactly
//!   one [`Event::Answer`], even once the run has withdrawn it
//!   ([`Request::Withdrawn`]); the run says it waits ([`Request::Waiting`])
//!   and tells facts ([`Request::Told`], best effort for the parent to
//!   forward); and the client hears at most one of [`Request::Finished`], how
//!   the run says it finishes, and [`Request::Faulted`], how the agent failed
//!   while its run was live. [`Event::Deliver`], [`Event::Answer`] and
//!   [`Event::Stop`] are notices, sent at any time while the client holds the
//!   agent's name: an agent that has gone drops them, as a stale handle is;
//!   one that has stopped talking to its run drops answers, bounces inbound
//!   events ([`Request::Bounced`]) and takes a stop as already asked. The
//!   watchdog pauses while a call waits for the client's answer, so what
//!   bounds a relayed call is the run's own deadline for it: past it, the
//!   run withdraws the call, and the client answers it at once.
//! - io's, through the protocol layer, which speaks the channel over the
//!   process's pipes and runs the process tree. A [`Request::Spawn`] is ended
//!   by exactly one [`Event::Spawned`] or [`Event::Unspawned`], by its
//!   deadline; a
//!   [`Request::Send`] by one [`Event::Sent`] or [`Event::Unsent`]; a
//!   [`Request::Read`], a demand for the next message up, by one
//!   [`Event::Received`], [`Event::Malformed`] or [`Event::Hangup`]; a
//!   [`Request::Signal`] by one [`Event::Signalled`]; a [`Request::Wait`] by
//!   one [`Event::Exited`]; and a [`Request::Reap`] by one [`Event::Reaped`],
//!   after the process's `Exited`, once its tree is empty. What the agent
//!   wrote before it went is still read, up to the channel's end; io releases
//!   what it holds for the process once its tree is empty and the channel up
//!   has been read to its end, which the agent sub-model always does. A
//!   request on a process io has released still gets its terminal, at once.
//!
//! A request's `owner` is the agent sub-model's token for the agent that
//! asked, echoed on its terminal; the variant says which request it ends, as
//! an agent has at most one of each in flight but signals. The client's
//! records carry its own token, `client`, and it addresses an agent by the
//! name `Started` gave it, `agent`. The parent addresses io's process by the
//! name `Spawned` gave, `process`.

use alloc::boxed::Box;

use temper_lib::{Time, Token};

use crate::channel::{Ask, Down, Finish, Reply, Up};

/// parent -> agent
#[derive(PartialEq, Eq, Debug)]
pub enum Event {
    /// From the client: spawn an agent process for `spawn`, and start its run.
    /// Ended by exactly one `Gone`.
    Spawn { client: Token, spawn: Spawn },
    /// From the client: an inbound event for the run of `agent`, which goes
    /// down as it arrives, or is bounced.
    Deliver { agent: Token, event: Box<[u8]> },
    /// From the client: the one answer to the host call `call` of the run of
    /// `agent`.
    Answer { agent: Token, call: Token, reply: Reply },
    /// From the client: stop `agent`. Its run is cancelled and winds down;
    /// past the grace its tree is terminated, then killed. Its `Gone` says
    /// when it has all gone.
    Stop { agent: Token },
    /// Terminal for `Spawn`: the process runs, and `process` names it from now
    /// on.
    Spawned { owner: Token, process: Token },
    /// Terminal for `Spawn`: the process could not be spawned by its
    /// deadline, and nothing of it is held. `detail` is for operators.
    Unspawned { owner: Token, detail: Box<[u8]> },
    /// Terminal for `Send`: the message went down.
    Sent { owner: Token },
    /// Terminal for `Send`: the message could not go down, as the agent no
    /// longer reads its channel.
    Unsent { owner: Token },
    /// Terminal for `Read`: the next message up.
    Received { owner: Token, message: Up },
    /// Terminal for `Read`: what came up is not a message the protocol layer
    /// can decode, or is larger than a message may be. Nothing more is read.
    Malformed { owner: Token },
    /// Terminal for `Read`: the agent's end of the channel closed.
    Hangup { owner: Token },
    /// Terminal for `Signal`, whatever the signal reached.
    Signalled { owner: Token },
    /// Terminal for `Wait`: the agent's process exited. Things it started may
    /// still run.
    Exited { owner: Token },
    /// Terminal for `Reap`: the process exited and its tree is empty, which io
    /// proves. `detail` is for operators, such as the tail of its error
    /// output.
    Reaped { owner: Token, detail: Box<[u8]> },
}

/// agent -> parent
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    /// To the client: the agent for `client` was spawned and its run started,
    /// and `agent` names it from now on.
    Started { client: Token, agent: Token },
    /// To the client: a host call of the run, `call` being the run's name for
    /// it. Answered by exactly one `Answer`.
    Called { client: Token, call: Token, ask: Ask },
    /// To the client: the run withdrew its call `call`, not yet answered. The
    /// client answers it all the same, once: at once as withdrawn, or with
    /// what came of it once that has settled.
    Withdrawn { client: Token, call: Token },
    /// To the client: the run waits for its next inbound event.
    Waiting { client: Token },
    /// To the client: a fact of the run, as it is, to forward best effort.
    Told { client: Token, fact: Box<[u8]> },
    /// To the client: how the run says it finishes. Its process exits next.
    Finished { client: Token, finish: Finish },
    /// To the client: the agent failed while its run was live, and is being
    /// stopped.
    Faulted { client: Token, fault: Fault },
    /// To the client: an inbound event was not passed on, for `bounce`.
    Bounced { client: Token, bounce: Bounce },
    /// To the client, terminal for `Spawn`: the agent has gone, for `end`.
    /// `detail` is for operators, never for an LLM.
    Gone { client: Token, end: End, detail: Box<[u8]> },
    /// Spawn the agent in a contained process tree, in the workspace
    /// `workspace`, where the client's repositories sit, giving up at
    /// `deadline`.
    Spawn { owner: Token, workspace: Token, deadline: Time },
    /// Send `message` down the channel of `process`.
    Send { owner: Token, process: Token, message: Down },
    /// Read the next message up the channel of `process`.
    Read { owner: Token, process: Token },
    /// Send `signal` to every member of the tree of `process`.
    Signal { owner: Token, process: Token, signal: Signal },
    /// Wait for `process` to exit.
    Wait { owner: Token, process: Token },
    /// Wait for the tree of `process` to be empty.
    Reap { owner: Token, process: Token },
}

/// What a client asks an agent to be spawned for.
#[derive(PartialEq, Eq, Hash, Debug)]
pub struct Spawn {
    /// Where the repositories sit: the client's name for the prepared
    /// workspace, which the layers below resolve. Passed through.
    pub workspace: Token,
    /// What the run is given, passed through.
    pub charter: Box<[u8]>,
    /// The state of a parked run to resume from, passed through.
    pub snapshot: Option<Box<[u8]>>,
}

/// How an agent failed (worker-model.md, 4.3).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Fault {
    /// It exited, or stopped reading or writing its channel, without saying
    /// how its run finishes.
    Exited,
    /// It broke the channel's rules, or said more than the limits allow.
    Rules,
    /// The watchdog stopped it: no progress.
    NoProgress,
    /// The watchdog stopped it: past its wall time.
    WallTime,
}

/// Why an inbound event was not passed on.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Bounce {
    /// It holds more bytes than the limits allow.
    TooLarge,
    /// As many events as may wait for the run are waiting already.
    Full,
    /// The run has stopped listening: it said how it finishes, its agent
    /// failed or is stopping.
    Ending,
}

/// How an agent ended.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum End {
    /// Refused at the entrance: every process slot is taken.
    Busy,
    /// Refused at the entrance: the spawn does not fit the limits.
    Invalid(Invalid),
    /// Its process could not be spawned.
    Unspawned,
    /// Its process ran, exited, and its tree is empty.
    Stopped,
}

/// What about a spawn does not fit the limits.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Invalid {
    /// The charter holds more bytes than a run may be given.
    Charter,
    /// The snapshot holds more bytes than a run may be given.
    Snapshot,
}

/// A signal for a process tree.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Signal {
    /// Asks every member to exit.
    Terminate,
    /// Ends every member.
    Kill,
}
