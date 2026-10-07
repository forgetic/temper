//! The records that cross the boundary with the protocol layer (4.4). The
//! domain defines them; the protocol crate depends on it.
//!
//! Four peers are behind it, and each record names which by its variant:
//!
//! - The engine link. A [`Request::Dial`] opens the one channel to the engine,
//!   and is ended by exactly one [`Event::Lost`], after an
//!   [`Event::Connected`] if the channel opened: a dial that fails is lost at
//!   once. The domain dials again on its own, and never more than one dial is
//!   in flight.
//! - The engine, over that channel: the host child domain's face
//!   (worker-domain.md, section 2). [`Request::Hello`] goes first on every
//!   channel. An [`Event::Assign`] is a call, answered under the run's and the
//!   attempt's names by a [`Request::Answer`] that the engine acknowledges
//!   ([`Event::Acknowledged`]). Until it does, the worker keeps the answer, and
//!   sends it again right after every hello, which lists its run as
//!   [`Phase::Answered`]; the engine drops one it has already, acknowledging it
//!   again. A refusal goes once, and holds no slot. The attempt hosted, or
//!   answered and not acknowledged, assigned again, is dropped: its one answer
//!   is the hosted run's. Everything else the engine sends names the run and
//!   the attempt, and is dropped unless that attempt is hosted (attempts are
//!   fenced). A [`Request::Relay`] is answered by exactly one
//!   [`Event::Relayed`] or [`Event::RelayCancelled`]; [`Request::Bounced`] is
//!   a notice. Either may still
//!   come after its attempt's answer, as may the run's facts ([`Told`]): the
//!   engine drops them.
//! - io's agent processes and their channels, as the agent child domain defines
//!   them (`temper_worker_domain_agent`): a [`Request::Spawn`] is ended by one
//!   [`Event::Spawned`] or [`Event::Unspawned`]; a [`Request::Send`] by one
//!   [`Event::Sent`] or [`Event::Unsent`]; a [`Request::Read`] by one
//!   [`Event::Received`], [`Event::Malformed`] or [`Event::Hangup`]; a
//!   [`Request::Signal`] by one [`Event::Signalled`]; a [`Request::Wait`] by
//!   one [`Event::Exited`]; a [`Request::Reap`] by one [`Event::Reaped`].
//! - io's git and file operations, as the checkout defines them
//!   (`temper_worker_domain_checkout::git`): a [`Request::Io`] is ended by one
//!   [`Event::Done`], with `Cancelled` if a [`Request::CancelIo`] won its
//!   race.
//!
//! The shell tells the domain to shut down ([`Event::Shutdown`]); the domain is
//! done once every run has answered and every answer has been delivered or,
//! with the engine out of reach past the grace, given up
//! ([`crate::Domain::is_done`]).
//!
//! A request's `owner` is the token of whoever asked, echoed on its terminal
//! event. Tokens of different families may be equal: the variant routes a
//! terminal to the child domain that asked.

use alloc::boxed::Box;

use crate::wire;
use skein_lib::{Duration, Time, Token};
use temper_worker_domain_agent::{self as agent, channel};
use temper_worker_domain_checkout::git;

/// protocol -> domain
#[derive(PartialEq, Eq, Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "fixed diagnostic tails keep boundary records bounded without allocation"
)]
pub enum Event {
    /// The engine's typed conversation state for an attempt.
    AssignTyped {
        assignment: wire::AssignmentTyped,
    },
    /// A named message with its sender label and words.
    InboundTyped {
        run: Token,
        attempt: Token,
        name: Token,
        sender: Box<[u8]>,
        words: Box<[u8]>,
    },
    /// A typed call's answer, under its opaque name and local delivery token.
    RelayedTyped {
        run: Token,
        attempt: Token,
        call: Box<[u8]>,
        delivery: Token,
        answer: Box<[u8]>,
    },
    ConnectedV2,
    /// Stable wire call name and the exact local delivery it answers.
    RelayedV2 {
        run: Token,
        attempt: Token,
        call: Token,
        delivery: Token,
        answer: Box<[u8]>,
    },
    AssignV2 {
        assignment: wire::AssignmentV2,
    },
    AcknowledgeTurn {
        run: Token,
        attempt: Token,
        turn: u32,
    },
    TurnBusy {
        run: Token,
        attempt: Token,
        turn: u32,
    },
    /// The channel to the engine opened: the worker says hello next.
    Connected,
    /// Terminal for `Dial`: the channel to the engine closed, or never opened.
    Lost,
    /// From the engine, a call: host the run of `assignment`, and answer once
    /// it has ended.
    Assign {
        assignment: wire::Assignment,
    },
    /// From the engine: an inbound event for the run `run`'s attempt
    /// `attempt`.
    Inbound {
        run: Token,
        attempt: Token,
        name: Token,
        event: Box<[u8]>,
    },
    /// From the engine: cancel the run `run`'s attempt `attempt`.
    Cancel {
        run: Token,
        attempt: Token,
    },
    Grant {
        run: Token,
        attempt: Token,
        grant: wire::Grant,
    },
    /// From the engine: the answer to the relayed call `call` of the run
    /// `run`'s attempt `attempt`.
    Relayed {
        run: Token,
        attempt: Token,
        call: Token,
        answer: Box<[u8]>,
    },
    /// Terminal for a cancelled relay delivery and wait.
    RelayCancelled {
        call: Token,
    },
    /// From the engine: it has the answer for the run `run`'s attempt
    /// `attempt`, which the worker forgets. One it has forgotten already, or
    /// a refusal's, changes nothing.
    Acknowledged {
        run: Token,
        attempt: Token,
    },
    /// From the shell: cancel every run, admit no more, and be done once every
    /// answer is delivered.
    Shutdown,
    /// Terminal for `Spawn`: the agent's process runs as `process`.
    Spawned {
        owner: Token,
        process: Token,
    },
    /// Terminal for `Spawn`: the process could not be spawned by its deadline.
    Unspawned {
        owner: Token,
        detail: Box<[u8]>,
    },
    /// Terminal for `Send`: the message went down.
    Sent {
        owner: Token,
    },
    /// Terminal for `Send`: the agent no longer reads its channel.
    Unsent {
        owner: Token,
    },
    /// Terminal for `Read`: the next message up.
    Received {
        owner: Token,
        message: channel::Up,
    },
    /// Terminal for `Read`: what came up is not a message.
    Malformed {
        owner: Token,
    },
    /// Terminal for `Read`: the agent's end of the channel closed.
    Hangup {
        owner: Token,
    },
    /// Terminal for `Signal`.
    Signalled {
        owner: Token,
    },
    /// Terminal for `Wait`: the agent's process exited.
    Exited {
        owner: Token,
    },
    /// Terminal for `Reap`: the process's tree is empty.
    Reaped {
        owner: Token,
        detail: Box<[u8]>,
    },
    /// Terminal for `Io`.
    Done {
        owner: Token,
        done: git::Done,
    },
}

/// domain -> protocol
#[derive(PartialEq, Eq, Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "fixed diagnostic tails keep boundary records bounded without allocation"
)]
pub enum Request {
    /// A named host call with the tool, write flag, input and time left.
    RelayTyped {
        run: Token,
        attempt: Token,
        call: Box<[u8]>,
        delivery: Token,
        tool: Box<[u8]>,
        writes: bool,
        input: Box<[u8]>,
        deadline: Duration,
    },
    HelloV2 {
        hello: Hello,
        graces: Duration,
        push_deadline: Duration,
    },
    Turn {
        run: Token,
        attempt: Token,
        turn: wire::Turn,
    },
    AnswerV2 {
        run: Token,
        attempt: Token,
        answer: wire::AnswerV2,
    },
    RelayV2 {
        run: Token,
        attempt: Token,
        call: Token,
        delivery: Token,
        body: Box<[u8]>,
    },
    /// Open the channel to the engine. Ended by one `Lost`, after a
    /// `Connected` if it opened.
    Dial,
    /// To the engine, first on every channel: what the worker is and hosts.
    Hello {
        hello: Hello,
    },
    /// To the engine, the answer to an `Assign`: exactly one per assignment.
    Answer {
        run: Token,
        attempt: Token,
        answer: wire::Answer,
    },
    /// To the engine: a host call of the run `run`'s attempt `attempt`, which
    /// the worker names `call`, relayed as it is.
    Relay {
        run: Token,
        attempt: Token,
        call: Token,
        body: Box<[u8]>,
    },
    /// Cancel a relay delivery and wait. Its original request still ends
    /// exactly once with `Relayed` or `RelayCancelled`; remote effects remain.
    CancelRelay {
        call: Token,
    },
    Rejected {
        run: Token,
        attempt: Token,
        account: u32,
        generation: u64,
    },
    Exhausted {
        run: Token,
        attempt: Token,
        account: u32,
        retry_after: Duration,
    },
    /// To the engine: an inbound event for the run `run`'s attempt `attempt`
    /// was not passed on, for `bounce`.
    Bounced {
        run: Token,
        attempt: Token,
        name: Token,
        bounce: wire::Bounce,
    },
    /// Spawn an agent in a contained process tree, in `workspace` when it has
    /// items, giving up at `deadline`.
    Spawn {
        owner: Token,
        workspace: Option<Token>,
        deadline: Time,
    },
    /// Send `message` down the channel of `process`.
    Send {
        owner: Token,
        process: Token,
        message: channel::Down,
    },
    /// Read the next message up the channel of `process`.
    Read {
        owner: Token,
        process: Token,
    },
    /// Send `signal` to every member of the tree of `process`.
    Signal {
        owner: Token,
        process: Token,
        signal: agent::Signal,
    },
    /// Wait for `process` to exit.
    Wait {
        owner: Token,
        process: Token,
    },
    /// Wait for the tree of `process` to be empty.
    Reap {
        owner: Token,
        process: Token,
    },
    /// Ask io for the git or file operation `op`, giving up at `deadline`.
    Io {
        owner: Token,
        op: git::Op,
        deadline: Time,
    },
    /// Abandon the `Io` in flight for `owner`. Its terminal event still comes:
    /// `Done` with `Cancelled`, or whichever outcome won the race.
    CancelIo {
        owner: Token,
    },
}

/// What the worker says first on every channel to the engine (engine-domain.md,
/// section 8), for the engine to place work and to keep or cancel each run it
/// hosts.
#[derive(PartialEq, Eq, Hash, Debug)]
pub struct Hello {
    /// How many runs it hosts at once.
    pub slots: u32,
    /// The workstreams it holds checkouts for, in byte order.
    pub workstreams: Box<[Box<[u8]>]>,
    /// The runs it hosts, in the order of the engine's names for them, then
    /// those whose answers it holds: none when it first dials in.
    pub hosting: Box<[Hosted]>,
}

/// A run the worker hosts, or whose answer it holds.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Hosted {
    pub run: Token,
    pub attempt: Token,
    pub phase: Phase,
}

/// Where a run is in its lifecycle (worker-domain.md, 4.2), as the hello says.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Phase {
    Preparing,
    Starting,
    /// Its agent is at work.
    Active,
    /// It yielded, and waits for its next inbound event.
    Waiting,
    /// How it ends is decided, and it answers next.
    Ending,
    /// It answered while there was no channel: the answer follows the hello.
    Answered,
}

/// A fact the run told, for the engine, as it is: forwarded best effort, for
/// liveness, operators and live views (worker-domain.md, section 7).
#[derive(PartialEq, Eq, Hash, Debug)]
pub struct Told {
    pub run: Token,
    pub attempt: Token,
    pub fact: Box<[u8]>,
}
