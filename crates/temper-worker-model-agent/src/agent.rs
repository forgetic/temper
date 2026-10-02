//! Agent processes: one per hosted run, each with one channel over its pipes
//! (worker-model.md, section 6).
//!
//! A `Spawn` takes a slot, or is refused at the entrance, and has io spawn
//! the agent in a contained process tree. Once it runs, its run's start
//! message goes down first and the run is live: inbound events and the
//! answers to its host calls go down as the client gives them, one message in
//! flight at a time and the rest waiting in order in the run's outbox; what
//! the run says comes up one message per read, checked against the channel's
//! rules (the `channel` module) as it comes. Host calls go to the client, and
//! so do the run's facts, its waiting, and how it finishes, after which
//! anything more it says breaks the rules. A call beyond the run's limit is
//! answered as busy at the entrance, and nothing more is read until that
//! answer is sent down, so the outbox always has room for what a message may
//! cause.
//!
//! The watchdog runs while the run is live. Every message counts as progress,
//! and silence for `Limits::no_progress` while its clock runs fails the agent.
//! The clock pauses while the run waits for an inbound event (from its
//! `Waiting` until the client delivers one) and while a call of its waits for
//! the client's answer, and runs afresh as it resumes; a long operation the
//! run reports holds it until the operation's deadline. The wall time runs
//! from the spawn, whatever the run is doing.
//!
//! Stopping is cancel, then kill. The client's stop sends the cancel down,
//! behind what waits, and the run winds down on its own within
//! `Limits::grace`; a run that says how it finishes is given the same grace to
//! exit. Past the grace the tree is terminated, and past `Limits::kill_after`
//! after that, killed. A fault (a broken rule, the watchdog, or a run that
//! stopped talking without saying how it finishes) terminates the tree at
//! once. A process that exits, or stops reading its channel, before its run
//! has said how it finishes is drained: what it wrote is read, a finish among
//! it, until the channel up ends or the grace is up.
//!
//! The client hears at most one of `Finished` and `Faulted`, and a fault only
//! while the run is live: once the client has stopped it, or the run has said
//! how it finishes, a fault still stops the agent, and only a fact tells of
//! it. A run that finishes after a stop is heard: how it finishes is its own
//! to say.
//!
//! An agent has gone, and the client is told so, only once its process has
//! exited and its tree is empty (io's `Exited` and `Reaped`), the channel up
//! has ended, and nothing it asked of io is in flight. Every state reads the
//! channel up to its end, so that the layers below can release it.
//!
//! A transition table, by state; what is not listed is unreachable by the
//! boundary's contract (one terminal per request, `Exited` before `Reaped`,
//! the client naming an agent only once it has started):
//!
//! ```text
//! state        event                          next          requests
//! -            spawn, no slot                 -             gone: busy
//!              spawn, beyond the limits       -             gone: invalid
//!              spawn                          Spawning      spawn
//! Spawning     spawned                        Live          started, wait, reap, send: start
//!              unspawned                      Closed        gone: unspawned
//! Live         received: a call               Live          called, or a busy answer down
//!              received: a fact               Live          told
//!              received: long                 Live
//!              received: waiting              Live          waiting
//!              received: finish               Exiting       finished
//!              received: a broken rule        Terminating   faulted: rules, terminate
//!              malformed                      Terminating   faulted: rules, terminate
//!              hangup                         Terminating   faulted: exited, terminate
//!              exited, unsent                 Draining
//!              deliver                        Live          (the event down), or bounced:
//!                                                             too large, full
//!              answer                         Live          (the answer down)
//!              stop                           Cancelled     (the cancel down)
//!              watchdog                       Terminating   faulted: no progress, terminate
//!              wall time                      Terminating   faulted: wall time, terminate
//! Cancelled    received: a call               Cancelled     called, or a busy answer down
//!              received: a fact               Cancelled     told
//!              received: long, waiting        Cancelled
//!              received: finish               Exiting       finished
//!              received: a broken rule        Terminating   terminate
//!              malformed                      Terminating   terminate
//!              hangup                         Exiting
//!              exited, unsent                 Draining
//!              deliver                        Cancelled     bounced: ending
//!              answer                         Cancelled     (the answer down)
//!              stop                           Cancelled
//!              grace                          Terminating   terminate
//! Draining     received: a fact               Draining      told
//!              received: a call, long,        Draining      (dropped: nothing hears its
//!                waiting                                      answer)
//!              received: finish               Exiting       finished
//!              received: a broken rule        Terminating   faulted: rules if live, terminate
//!              malformed                      Terminating   faulted: rules if live, terminate
//!              hangup                         Exiting       faulted: exited if live
//!              deliver                        Draining      bounced: ending
//!              answer                         Draining      (dropped)
//!              stop                           Draining      (no longer live)
//!              grace                          Terminating   faulted: exited if live, terminate
//! Exiting      received, malformed            Terminating   terminate (it broke the rules)
//!              hangup, exited                 Exiting
//!              deliver                        Exiting       bounced: ending
//!              answer, stop                   Exiting       (dropped)
//!              grace                          Terminating   terminate
//! Terminating  received                       Terminating   (dropped)
//!              malformed, hangup, exited      Terminating
//!              deliver                        Terminating   bounced: ending
//!              answer, stop                   Terminating   (dropped)
//!              grace                          Killing       kill
//! Killing      as Terminating, with no grace  Killing
//! ```
//!
//! io's terminals are recorded on the process in every state that has one,
//! before the transition: a send or a read ended (`sent`, `unsent`, the
//! read's three), a signal ended, the process exited, its tree reaped.
//! What a state implies follows from it, in one place ([`follow`]): the next
//! message down and the next read, the agent's end once it has settled, its
//! alarms (the watchdog and the wall time while live, the grace while
//! stopping until it is killed), and its retirement once it is Closed.

use alloc::boxed::Box;
use core::mem;

use temper_lib::bytes::copy_of;
use temper_lib::{Deadlines, Env, Id, Queue, Set, Slab, Time, Token};

use crate::boundary::{Bounce, End, Fault, Invalid, Request, Signal, Spawn};
use crate::channel::{Ask, Down, Finish, Reply, Up};
use crate::facts::{Fact, Facts};
use crate::limits::{self, Limits};
use crate::model::Model;

#[derive(Debug)]
pub(crate) struct Agent {
    /// The client's token, echoed on every record back to it.
    client: Token,
    state: State,
}

/// The agent sub-model's alarms, each for one agent.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(crate) enum Alarm {
    /// The live run has gone without progress for as long as it may.
    Watchdog { agent: Id<Agent> },
    /// The live run's wall time is up.
    Wall { agent: Id<Agent> },
    /// The grace of a stopping agent is up.
    Grace { agent: Id<Agent> },
}

#[derive(Debug)]
enum State {
    /// Its process is being spawned; what its run starts with waits for it.
    Spawning { charter: Box<[u8]>, snapshot: Option<Box<[u8]>> },
    /// Its run is live: the channel is open both ways, and the watchdog and
    /// the wall time run.
    Live { process: Process, channel: Channel, watch: Watch },
    /// The client stopped it: the cancel goes down, and the run winds down on
    /// its own until `until`.
    Cancelled { process: Process, channel: Channel, until: Time },
    /// Its process exited, or stopped reading its channel, before its run said
    /// how it finishes: what it wrote is read until `until`. `live` while the
    /// client has not stopped it, and the agent's fault is told if no finish
    /// comes.
    Draining { process: Process, until: Time, live: bool },
    /// Its run has nothing more to say: it said how it finishes, or its
    /// channel ended once it was stopped. Its process goes on its own until
    /// `until`.
    Exiting { process: Process, until: Time },
    /// Its tree was told to exit, and is killed at `until`.
    Terminating { process: Process, until: Time },
    /// Its tree was killed.
    Killing { process: Process },
    /// Terminal: holds nothing.
    Closed,
}

/// What an agent asked of io for its process, and what io said of it.
#[derive(Debug)]
struct Process {
    /// io's name for the process.
    name: Token,
    reading: Reading,
    /// A send is in flight.
    sending: bool,
    /// Signals in flight.
    signals: u32,
    exited: bool,
    /// Once its tree is empty, the detail of its end.
    reaped: Option<Box<[u8]>>,
}

/// The channel up, as it is read.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Reading {
    /// No read is in flight.
    Idle,
    /// A read is in flight.
    Asked,
    /// The channel up has ended: hung up, or malformed. Nothing more is read.
    Ended,
}

/// The channel while the run listens.
#[derive(Debug)]
struct Channel {
    /// Messages waiting to go down behind the one in flight, in order: the
    /// inbound events that may wait, an answer per call in flight, the
    /// cancel.
    outbox: Queue<Down>,
    /// Inbound events among them.
    events: u32,
    /// A call answered as busy at the entrance: its answer goes down next, and
    /// nothing more is read until it has.
    busy: Option<Token>,
    /// The names of the run's calls in flight, from the call until its answer
    /// is sent down.
    flight: Set<Token>,
    /// Those the client has not answered yet.
    asked: Set<Token>,
}

/// The watchdog's view of a live run.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Watch {
    /// When it last made progress, or its clock last resumed.
    seen: Time,
    /// Until when a long operation it reported may run.
    held: Time,
    /// It waits for its next inbound event.
    waiting: bool,
    /// When its wall time is up.
    wall: Time,
}

/// The tokens a cell handler names an agent by: its own, which io echoes, and
/// its client's.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Names {
    owner: Token,
    client: Token,
}

// Entry points, one per event or alarm: each records what io said on the
// process, moves the state through its cell's handler, and follows the new
// state ([`follow`]).

pub(crate) fn spawn(model: &mut Model, env: &Env<Limits>, client: Token, spawn: Spawn, out: &mut Queue<Request>) {
    let Spawn { workspace, charter, snapshot } = spawn;
    let limits = &env.limits;
    let refusal = if model.agents.is_full() {
        Some(End::Busy)
    } else if !within(&charter, limits.charter_bytes) {
        Some(End::Invalid(Invalid::Charter))
    } else if !within_optional(snapshot.as_deref(), limits.snapshot_bytes) {
        Some(End::Invalid(Invalid::Snapshot))
    } else {
        None
    };
    if let Some(end) = refusal {
        model.facts.push(Fact::Gone { client, end });
        out.push(Request::Gone { client, end, detail: Box::new([]) });
        return;
    }
    let agent = Agent { client, state: State::Spawning { charter, snapshot } };
    let id = model.agents.insert(agent).expect("checked for room above");
    out.push(Request::Spawn { owner: id.token(), workspace });
}

pub(crate) fn deliver(model: &mut Model, env: &Env<Limits>, agent: Token, event: Box<[u8]>, out: &mut Queue<Request>) {
    let Some(id) = addressed(&model.agents, agent) else {
        return;
    };
    let entry = model.agents.get_mut(id).expect("addressed above");
    let client = entry.client;
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Live { process, channel, watch } => delivered(client, process, channel, watch, event, env, out),
        // It no longer listens.
        state @ (State::Cancelled { .. }
        | State::Draining { .. }
        | State::Exiting { .. }
        | State::Terminating { .. }
        | State::Killing { .. }) => {
            out.push(Request::Bounced { client, bounce: Bounce::Ending });
            state
        }
        State::Spawning { .. } | State::Closed => unreachable!("an addressed agent has started and not gone"),
    };
    follow(model, env, id, out);
}

pub(crate) fn answer(
    model: &mut Model,
    env: &Env<Limits>,
    agent: Token,
    call: Token,
    reply: Reply,
    out: &mut Queue<Request>,
) {
    let Some(id) = addressed(&model.agents, agent) else {
        return;
    };
    let entry = model.agents.get_mut(id).expect("addressed above");
    let reply = bounded(reply, &env.limits);
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Live { process, mut channel, watch } => {
            answered(&mut channel, call, reply);
            // The clock resumes once no call waits for the client.
            let watch = if channel.asked.is_empty() { Watch { seen: env.now, ..watch } } else { watch };
            State::Live { process, channel, watch }
        }
        State::Cancelled { process, mut channel, until } => {
            answered(&mut channel, call, reply);
            State::Cancelled { process, channel, until }
        }
        // It no longer listens: the answer is dropped.
        state
        @ (State::Draining { .. } | State::Exiting { .. } | State::Terminating { .. } | State::Killing { .. }) => state,
        State::Spawning { .. } | State::Closed => unreachable!("an addressed agent has started and not gone"),
    };
    follow(model, env, id, out);
}

pub(crate) fn stop(model: &mut Model, env: &Env<Limits>, agent: Token, out: &mut Queue<Request>) {
    let Some(id) = addressed(&model.agents, agent) else {
        return;
    };
    let Model { agents, alarms: _, facts } = model;
    let entry = agents.get_mut(id).expect("addressed above");
    let client = entry.client;
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Live { process, channel, watch: _ } => cancel(client, process, channel, env, facts),
        State::Draining { process, until, live: _ } => State::Draining { process, until, live: false },
        // It is stopping already.
        state @ (State::Cancelled { .. }
        | State::Exiting { .. }
        | State::Terminating { .. }
        | State::Killing { .. }) => state,
        State::Spawning { .. } | State::Closed => unreachable!("an addressed agent has started and not gone"),
    };
    follow(model, env, id, out);
}

pub(crate) fn spawned(model: &mut Model, env: &Env<Limits>, owner: Token, process: Token, out: &mut Queue<Request>) {
    let Model { agents, alarms: _, facts } = model;
    let id = Id::<Agent>::from_token(owner);
    let entry = agents.get_mut(id).expect("an agent lives until its requests have ended");
    let names = Names { owner, client: entry.client };
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Spawning { charter, snapshot } => start(names, process, charter, snapshot, env, facts, out),
        State::Live { .. }
        | State::Cancelled { .. }
        | State::Draining { .. }
        | State::Exiting { .. }
        | State::Terminating { .. }
        | State::Killing { .. }
        | State::Closed => unreachable!("only a spawning agent's spawn ends"),
    };
    follow(model, env, id, out);
}

pub(crate) fn unspawned(
    model: &mut Model,
    env: &Env<Limits>,
    owner: Token,
    detail: Box<[u8]>,
    out: &mut Queue<Request>,
) {
    let Model { agents, alarms: _, facts } = model;
    let id = Id::<Agent>::from_token(owner);
    let entry = agents.get_mut(id).expect("an agent lives until its requests have ended");
    let client = entry.client;
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Spawning { charter: _, snapshot: _ } => {
            facts.push(Fact::Gone { client, end: End::Unspawned });
            out.push(Request::Gone { client, end: End::Unspawned, detail: tail(detail, &env.limits) });
            State::Closed
        }
        State::Live { .. }
        | State::Cancelled { .. }
        | State::Draining { .. }
        | State::Exiting { .. }
        | State::Terminating { .. }
        | State::Killing { .. }
        | State::Closed => unreachable!("only a spawning agent's spawn ends"),
    };
    follow(model, env, id, out);
}

pub(crate) fn sent(model: &mut Model, env: &Env<Limits>, owner: Token, out: &mut Queue<Request>) {
    let id = Id::<Agent>::from_token(owner);
    let entry = model.agents.get_mut(id).expect("an agent lives until its requests have ended");
    let process = process_of(&mut entry.state);
    assert!(process.sending, "a send ends once");
    process.sending = false;
    follow(model, env, id, out);
}

pub(crate) fn unsent(model: &mut Model, env: &Env<Limits>, owner: Token, out: &mut Queue<Request>) {
    let id = Id::<Agent>::from_token(owner);
    let entry = model.agents.get_mut(id).expect("an agent lives until its requests have ended");
    let process = process_of(&mut entry.state);
    assert!(process.sending, "a send ends once");
    process.sending = false;
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        // It no longer reads its channel: what it wrote is drained.
        State::Live { process, channel: _, watch: _ } => {
            State::Draining { process, until: env.now.saturating_add(env.limits.grace), live: true }
        }
        State::Cancelled { process, channel: _, until } => State::Draining { process, until, live: false },
        state
        @ (State::Draining { .. } | State::Exiting { .. } | State::Terminating { .. } | State::Killing { .. }) => state,
        State::Spawning { .. } | State::Closed => unreachable!("only a spawned agent sends"),
    };
    follow(model, env, id, out);
}

pub(crate) fn received(model: &mut Model, env: &Env<Limits>, owner: Token, message: Up, out: &mut Queue<Request>) {
    let Model { agents, alarms: _, facts } = model;
    let id = Id::<Agent>::from_token(owner);
    let entry = agents.get_mut(id).expect("an agent lives until its requests have ended");
    let names = Names { owner, client: entry.client };
    let process = process_of(&mut entry.state);
    assert!(process.reading == Reading::Asked, "a read ends once");
    process.reading = Reading::Idle;
    let limits = &env.limits;
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Live { process, channel, watch } => {
            if broken(&message, Some(&channel.flight), limits) {
                fail(names, process, Fault::Rules, true, env, facts, out)
            } else {
                heard(names.client, process, channel, watch, message, env, facts, out)
            }
        }
        State::Cancelled { process, channel, until } => {
            if broken(&message, Some(&channel.flight), limits) {
                fail(names, process, Fault::Rules, false, env, facts, out)
            } else {
                wound(names.client, process, channel, until, message, facts, out)
            }
        }
        State::Draining { process, until, live } => {
            if broken(&message, None, limits) {
                fail(names, process, Fault::Rules, live, env, facts, out)
            } else {
                drained(names.client, process, until, live, message, facts, out)
            }
        }
        // Its run has said all it may: anything more breaks the rules.
        State::Exiting { process, until: _ } => fail(names, process, Fault::Rules, false, env, facts, out),
        // What a tree being stopped writes is read to the end, and dropped.
        state @ (State::Terminating { .. } | State::Killing { .. }) => state,
        State::Spawning { .. } | State::Closed => unreachable!("only a spawned agent reads"),
    };
    follow(model, env, id, out);
}

pub(crate) fn malformed(model: &mut Model, env: &Env<Limits>, owner: Token, out: &mut Queue<Request>) {
    let Model { agents, alarms: _, facts } = model;
    let id = Id::<Agent>::from_token(owner);
    let entry = agents.get_mut(id).expect("an agent lives until its requests have ended");
    let names = Names { owner, client: entry.client };
    end_read(process_of(&mut entry.state));
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Live { process, .. } => fail(names, process, Fault::Rules, true, env, facts, out),
        State::Draining { process, until: _, live } => fail(names, process, Fault::Rules, live, env, facts, out),
        State::Cancelled { process, .. } | State::Exiting { process, .. } => {
            fail(names, process, Fault::Rules, false, env, facts, out)
        }
        state @ (State::Terminating { .. } | State::Killing { .. }) => state,
        State::Spawning { .. } | State::Closed => unreachable!("only a spawned agent reads"),
    };
    follow(model, env, id, out);
}

pub(crate) fn hangup(model: &mut Model, env: &Env<Limits>, owner: Token, out: &mut Queue<Request>) {
    let Model { agents, alarms: _, facts } = model;
    let id = Id::<Agent>::from_token(owner);
    let entry = agents.get_mut(id).expect("an agent lives until its requests have ended");
    let names = Names { owner, client: entry.client };
    end_read(process_of(&mut entry.state));
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        // It stopped talking without saying how its run finishes.
        State::Live { process, .. } => fail(names, process, Fault::Exited, true, env, facts, out),
        State::Cancelled { process, channel: _, until } => State::Exiting { process, until },
        State::Draining { process, until, live } => {
            if live {
                fault(names.client, Fault::Exited, true, facts, out);
            }
            State::Exiting { process, until }
        }
        state @ (State::Exiting { .. } | State::Terminating { .. } | State::Killing { .. }) => state,
        State::Spawning { .. } | State::Closed => unreachable!("only a spawned agent reads"),
    };
    follow(model, env, id, out);
}

pub(crate) fn signalled(model: &mut Model, env: &Env<Limits>, owner: Token, out: &mut Queue<Request>) {
    let id = Id::<Agent>::from_token(owner);
    let entry = model.agents.get_mut(id).expect("an agent lives until its requests have ended");
    let process = process_of(&mut entry.state);
    process.signals = process.signals.checked_sub(1).expect("a signal ends once");
    follow(model, env, id, out);
}

pub(crate) fn exited(model: &mut Model, env: &Env<Limits>, owner: Token, out: &mut Queue<Request>) {
    let id = Id::<Agent>::from_token(owner);
    let entry = model.agents.get_mut(id).expect("an agent lives until its requests have ended");
    let process = process_of(&mut entry.state);
    assert!(!process.exited, "a wait ends once");
    process.exited = true;
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        // What it wrote before it went is drained.
        State::Live { process, channel: _, watch: _ } => {
            State::Draining { process, until: env.now.saturating_add(env.limits.grace), live: true }
        }
        State::Cancelled { process, channel: _, until } => State::Draining { process, until, live: false },
        state
        @ (State::Draining { .. } | State::Exiting { .. } | State::Terminating { .. } | State::Killing { .. }) => state,
        State::Spawning { .. } | State::Closed => unreachable!("only a spawned agent is waited for"),
    };
    follow(model, env, id, out);
}

pub(crate) fn reaped(model: &mut Model, env: &Env<Limits>, owner: Token, detail: Box<[u8]>, out: &mut Queue<Request>) {
    let id = Id::<Agent>::from_token(owner);
    let entry = model.agents.get_mut(id).expect("an agent lives until its requests have ended");
    let process = process_of(&mut entry.state);
    assert!(process.exited, "io reports a process's exit before its reap");
    assert!(process.reaped.is_none(), "a reap ends once");
    process.reaped = Some(tail(detail, &env.limits));
    follow(model, env, id, out);
}

pub(crate) fn watchdog(model: &mut Model, env: &Env<Limits>, id: Id<Agent>, out: &mut Queue<Request>) {
    expired(model, env, id, Fault::NoProgress, out);
}

pub(crate) fn wall(model: &mut Model, env: &Env<Limits>, id: Id<Agent>, out: &mut Queue<Request>) {
    expired(model, env, id, Fault::WallTime, out);
}

pub(crate) fn grace(model: &mut Model, env: &Env<Limits>, id: Id<Agent>, out: &mut Queue<Request>) {
    let Model { agents, alarms: _, facts } = model;
    let entry = agents.get_mut(id).expect("an agent's alarms are cancelled before it is retired");
    let names = Names { owner: id.token(), client: entry.client };
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Cancelled { process, .. } | State::Exiting { process, .. } => terminate(names, process, env, facts, out),
        State::Draining { process, until: _, live } => {
            if live {
                fault(names.client, Fault::Exited, true, facts, out);
            }
            terminate(names, process, env, facts, out)
        }
        State::Terminating { process, until: _ } => kill(names, process, facts, out),
        State::Spawning { .. } | State::Live { .. } | State::Killing { .. } | State::Closed => {
            unreachable!("the grace runs only while an agent is stopping, until it is killed")
        }
    };
    follow(model, env, id, out);
}

/// The watchdog or the wall time, firing while the run is live: the agent
/// failed for `fault`.
fn expired(model: &mut Model, env: &Env<Limits>, id: Id<Agent>, fault: Fault, out: &mut Queue<Request>) {
    let Model { agents, alarms: _, facts } = model;
    let entry = agents.get_mut(id).expect("an agent's alarms are cancelled before it is retired");
    let names = Names { owner: id.token(), client: entry.client };
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Live { process, .. } => fail(names, process, fault, true, env, facts, out),
        State::Spawning { .. }
        | State::Cancelled { .. }
        | State::Draining { .. }
        | State::Exiting { .. }
        | State::Terminating { .. }
        | State::Killing { .. }
        | State::Closed => unreachable!("the watchdog and the wall time run only while the run is live"),
    };
    follow(model, env, id, out);
}

/// The agent a client's handle names, or `None` if it has gone: the handle
/// travelled down while the agent's `Gone` travelled up, and is dropped (5.2).
fn addressed(agents: &Slab<Agent>, agent: Token) -> Option<Id<Agent>> {
    let id = Id::from_token(agent);
    match &agents.get(id)?.state {
        State::Live { .. }
        | State::Cancelled { .. }
        | State::Draining { .. }
        | State::Exiting { .. }
        | State::Terminating { .. }
        | State::Killing { .. } => Some(id),
        State::Spawning { .. } => unreachable!("a client names an agent only once it has started"),
        State::Closed => None,
    }
}

/// The process of a spawned agent, for io's terminals.
fn process_of(state: &mut State) -> &mut Process {
    match state {
        State::Live { process, .. }
        | State::Cancelled { process, .. }
        | State::Draining { process, .. }
        | State::Exiting { process, .. }
        | State::Terminating { process, .. }
        | State::Killing { process } => process,
        State::Spawning { .. } | State::Closed => {
            unreachable!("io ends requests only for a spawned agent, which lives until they have ended")
        }
    }
}

/// A read ended the channel up: nothing more is read.
fn end_read(process: &mut Process) {
    assert!(process.reading == Reading::Asked, "a read ends once");
    process.reading = Reading::Ended;
}

/// Applied after every transition: the channel's flow, the agent's end once
/// it has settled, then its alarms, and its retirement once it is Closed.
fn follow(model: &mut Model, env: &Env<Limits>, id: Id<Agent>, out: &mut Queue<Request>) {
    let Model { agents, alarms, facts } = model;
    let entry = agents.get_mut(id).expect("an agent lives until it is retired");
    flow(id.token(), &mut entry.state, out);
    settle(entry, facts, out);
    let (watchdog, wall, grace) = timers(&entry.state, &env.limits);
    set(alarms, Alarm::Watchdog { agent: id }, watchdog);
    set(alarms, Alarm::Wall { agent: id }, wall);
    set(alarms, Alarm::Grace { agent: id }, grace);
    let closed = match &entry.state {
        State::Closed => true,
        State::Spawning { .. }
        | State::Live { .. }
        | State::Cancelled { .. }
        | State::Draining { .. }
        | State::Exiting { .. }
        | State::Terminating { .. }
        | State::Killing { .. } => false,
    };
    if closed {
        agents.retire(id);
    }
}

/// What a state implies for the channel: while the run listens, the next
/// message waiting goes down once none is in flight, a busy answer first; and
/// in every state the next message up is read once none is being read, until
/// the channel up ends, but while a busy answer waits.
fn flow(owner: Token, state: &mut State, out: &mut Queue<Request>) {
    let (process, reads) = match state {
        State::Live { process, channel, .. } | State::Cancelled { process, channel, .. } => {
            send_next(owner, process, channel, out);
            (process, channel.busy.is_none())
        }
        State::Draining { process, .. }
        | State::Exiting { process, .. }
        | State::Terminating { process, .. }
        | State::Killing { process } => (process, true),
        State::Spawning { .. } | State::Closed => return,
    };
    if reads && process.reading == Reading::Idle {
        out.push(Request::Read { owner, process: process.name });
        process.reading = Reading::Asked;
    }
}

fn send_next(owner: Token, process: &mut Process, channel: &mut Channel, out: &mut Queue<Request>) {
    if process.sending {
        return;
    }
    let message = match channel.busy.take() {
        Some(call) => Down::Answer { call, reply: Reply::Busy },
        None => {
            let Some(message) = channel.outbox.pop() else {
                return;
            };
            leaving(channel, &message);
            message
        }
    };
    out.push(Request::Send { owner, process: process.name, message });
    process.sending = true;
}

/// What leaves the outbox for the channel: an event no longer waits, and an
/// answer's call is no longer in flight.
fn leaving(channel: &mut Channel, message: &Down) {
    match message {
        Down::Event { .. } => channel.events = channel.events.checked_sub(1).expect("the events waiting are counted"),
        Down::Answer { call, reply: _ } => {
            let flying = channel.flight.remove(call);
            assert!(flying, "a call is in flight until its answer goes down");
        }
        Down::Cancel => {}
        Down::Start { .. } => unreachable!("the start message goes down first, never from the outbox"),
    }
}

/// An agent whose run has nothing more to say has gone once its process has:
/// exited, its tree empty, the channel up ended, and nothing asked of io in
/// flight. The client is told, with the detail of its end.
fn settle(entry: &mut Agent, facts: &mut Facts, out: &mut Queue<Request>) {
    let settled = match &entry.state {
        State::Exiting { process, .. } | State::Terminating { process, .. } | State::Killing { process } => {
            gone(process)
        }
        State::Spawning { .. }
        | State::Live { .. }
        | State::Cancelled { .. }
        | State::Draining { .. }
        | State::Closed => false,
    };
    if !settled {
        return;
    }
    let state = mem::replace(&mut entry.state, State::Closed);
    let detail = match state {
        State::Exiting { process, .. } | State::Terminating { process, .. } | State::Killing { process } => {
            process.reaped.expect("a process that has gone was reaped")
        }
        State::Spawning { .. }
        | State::Live { .. }
        | State::Cancelled { .. }
        | State::Draining { .. }
        | State::Closed => unreachable!("only a stopping agent settles"),
    };
    let client = entry.client;
    facts.push(Fact::Gone { client, end: End::Stopped });
    out.push(Request::Gone { client, end: End::Stopped, detail });
}

fn gone(process: &Process) -> bool {
    process.exited
        && process.reaped.is_some()
        && process.reading == Reading::Ended
        && !process.sending
        && process.signals == 0
}

/// When a state's alarms fall due: the watchdog and the wall time while the
/// run is live (the watchdog only while its clock runs, from the later of its
/// last progress and a long operation's deadline), the grace while it is
/// stopping, until it is killed.
fn timers(state: &State, limits: &Limits) -> (Option<Time>, Option<Time>, Option<Time>) {
    match state {
        State::Live { process: _, channel, watch } => {
            let runs = channel.asked.is_empty() && !watch.waiting;
            let due = watch.seen.max(watch.held).saturating_add(limits.no_progress);
            (runs.then_some(due), Some(watch.wall), None)
        }
        State::Cancelled { until, .. }
        | State::Draining { until, .. }
        | State::Exiting { until, .. }
        | State::Terminating { until, .. } => (None, None, Some(*until)),
        State::Spawning { .. } | State::Killing { .. } | State::Closed => (None, None, None),
    }
}

fn set(alarms: &mut Deadlines<Alarm>, alarm: Alarm, at: Option<Time>) {
    if let Some(at) = at {
        alarms.arm(alarm, at).expect("the alarm table has room for two alarms per agent");
    } else {
        alarms.cancel(alarm);
    }
}

// Cell handlers: each takes the source state's data by value and returns the
// target state.

/// Spawning, spawned: the run starts, its start message going down first.
fn start(
    names: Names,
    process: Token,
    charter: Box<[u8]>,
    snapshot: Option<Box<[u8]>>,
    env: &Env<Limits>,
    facts: &mut Facts,
    out: &mut Queue<Request>,
) -> State {
    let (owner, client, limits, now) = (names.owner, names.client, &env.limits, env.now);
    out.push(Request::Started { client, agent: owner });
    out.push(Request::Wait { owner, process });
    out.push(Request::Reap { owner, process });
    out.push(Request::Send { owner, process, message: Down::Start { charter, snapshot } });
    facts.push(Fact::Started { client });
    let outbox = limits::outbox(limits).expect("worst_case accepted the limits");
    let process =
        Process { name: process, reading: Reading::Idle, sending: true, signals: 0, exited: false, reaped: None };
    let channel = Channel {
        outbox: Queue::with_capacity(outbox),
        events: 0,
        busy: None,
        flight: Set::with_capacity(limits.calls),
        asked: Set::with_capacity(limits.calls),
    };
    let watch = Watch { seen: now, held: now, waiting: false, wall: now.saturating_add(limits.wall_time) };
    State::Live { process, channel, watch }
}

/// Live, deliver: the event waits to go down, or is bounced; a run that
/// waited for it resumes its clock.
fn delivered(
    client: Token,
    process: Process,
    mut channel: Channel,
    watch: Watch,
    event: Box<[u8]>,
    env: &Env<Limits>,
    out: &mut Queue<Request>,
) -> State {
    let limits = &env.limits;
    if !within(&event, limits.event_bytes) {
        out.push(Request::Bounced { client, bounce: Bounce::TooLarge });
        return State::Live { process, channel, watch };
    }
    if channel.events >= limits.events {
        out.push(Request::Bounced { client, bounce: Bounce::Full });
        return State::Live { process, channel, watch };
    }
    channel.outbox.push(Down::Event { event });
    channel.events = channel.events.checked_add(1).expect("no more events wait than the limits allow");
    let watch = if watch.waiting { Watch { seen: env.now, waiting: false, ..watch } } else { watch };
    State::Live { process, channel, watch }
}

/// The client's answer to a call of the run waits to go down.
fn answered(channel: &mut Channel, call: Token, reply: Reply) {
    let asked = channel.asked.remove(&call);
    assert!(asked, "the client answers each call it was told of, once");
    channel.outbox.push(Down::Answer { call, reply });
}

/// Live, stop: the cancel goes down behind what waits.
fn cancel(client: Token, process: Process, mut channel: Channel, env: &Env<Limits>, facts: &mut Facts) -> State {
    channel.outbox.push(Down::Cancel);
    facts.push(Fact::Cancelled { client });
    State::Cancelled { process, channel, until: env.now.saturating_add(env.limits.grace) }
}

/// Live, received a message within the rules: whatever it is, it is progress.
#[expect(clippy::too_many_arguments, reason = "a cell handler takes the fields it touches")]
fn heard(
    client: Token,
    process: Process,
    mut channel: Channel,
    mut watch: Watch,
    message: Up,
    env: &Env<Limits>,
    facts: &mut Facts,
    out: &mut Queue<Request>,
) -> State {
    let now = env.now;
    watch.seen = now;
    match message {
        Up::Call { call, ask } => called(client, &mut channel, call, ask, out),
        Up::Fact { fact } => out.push(Request::Told { client, fact }),
        Up::Long { span } => watch.held = watch.held.max(now.saturating_add(span)),
        Up::Waiting => {
            watch.waiting = true;
            out.push(Request::Waiting { client });
        }
        Up::Finish { finish } => {
            return finished(client, process, finish, now.saturating_add(env.limits.grace), facts, out);
        }
    }
    State::Live { process, channel, watch }
}

/// Cancelled, received a message within the rules: the run winds down, and
/// the watchdog no longer runs.
fn wound(
    client: Token,
    process: Process,
    mut channel: Channel,
    until: Time,
    message: Up,
    facts: &mut Facts,
    out: &mut Queue<Request>,
) -> State {
    match message {
        Up::Call { call, ask } => called(client, &mut channel, call, ask, out),
        Up::Fact { fact } => out.push(Request::Told { client, fact }),
        Up::Long { span: _ } | Up::Waiting => {}
        Up::Finish { finish } => return finished(client, process, finish, until, facts, out),
    }
    State::Cancelled { process, channel, until }
}

/// Draining, received a message within the rules: what the run wrote before
/// it went. Nothing would hear an answer to a call.
fn drained(
    client: Token,
    process: Process,
    until: Time,
    live: bool,
    message: Up,
    facts: &mut Facts,
    out: &mut Queue<Request>,
) -> State {
    match message {
        Up::Call { .. } | Up::Long { span: _ } | Up::Waiting => {}
        Up::Fact { fact } => out.push(Request::Told { client, fact }),
        Up::Finish { finish } => return finished(client, process, finish, until, facts, out),
    }
    State::Draining { process, until, live }
}

/// A host call within the rules goes to the client, or, if the run has as
/// many in flight as it may, is answered as busy.
fn called(client: Token, channel: &mut Channel, call: Token, ask: Ask, out: &mut Queue<Request>) {
    assert!(channel.busy.is_none(), "nothing is read while a busy answer waits to go down");
    match channel.flight.insert(call) {
        Ok(fresh) => {
            assert!(fresh, "a call reusing a name in flight breaks the rules");
            let asked = channel.asked.insert(call);
            assert!(asked == Ok(true), "the calls asked are among those in flight");
            out.push(Request::Called { client, call, ask });
        }
        Err(call) => channel.busy = Some(call),
    }
}

/// The run said how it finishes: its last word. Its process exits on its own
/// until `until`.
fn finished(
    client: Token,
    process: Process,
    finish: Finish,
    until: Time,
    facts: &mut Facts,
    out: &mut Queue<Request>,
) -> State {
    facts.push(Fact::Finished { client });
    out.push(Request::Finished { client, finish });
    State::Exiting { process, until }
}

/// The agent failed for `fault`, told to the client if `told`: its tree is
/// terminated.
fn fail(
    names: Names,
    process: Process,
    fault: Fault,
    told: bool,
    env: &Env<Limits>,
    facts: &mut Facts,
    out: &mut Queue<Request>,
) -> State {
    self::fault(names.client, fault, told, facts, out);
    terminate(names, process, env, facts, out)
}

fn fault(client: Token, fault: Fault, told: bool, facts: &mut Facts, out: &mut Queue<Request>) {
    facts.push(Fact::Faulted { client, fault });
    if told {
        out.push(Request::Faulted { client, fault });
    }
}

/// Its tree is told to exit, and killed past `Limits::kill_after`.
fn terminate(
    names: Names,
    mut process: Process,
    env: &Env<Limits>,
    facts: &mut Facts,
    out: &mut Queue<Request>,
) -> State {
    signal(names.owner, &mut process, Signal::Terminate, out);
    facts.push(Fact::Terminated { client: names.client });
    State::Terminating { process, until: env.now.saturating_add(env.limits.kill_after) }
}

/// Terminating, grace: its tree is killed.
fn kill(names: Names, mut process: Process, facts: &mut Facts, out: &mut Queue<Request>) -> State {
    signal(names.owner, &mut process, Signal::Kill, out);
    facts.push(Fact::Killed { client: names.client });
    State::Killing { process }
}

fn signal(owner: Token, process: &mut Process, signal: Signal, out: &mut Queue<Request>) {
    out.push(Request::Signal { owner, process: process.name, signal });
    process.signals = process.signals.checked_add(1).expect("a tree is signalled twice at most");
}

/// Whether a message breaks the channel's rules: a payload beyond the limits,
/// or, while the run's calls are in `flight`, a call reusing a name in flight.
fn broken(message: &Up, flight: Option<&Set<Token>>, limits: &Limits) -> bool {
    match message {
        Up::Call { call, ask } => {
            let reused = match flight {
                Some(flight) => flight.contains(call),
                None => false,
            };
            let body = match ask {
                Ask::Push { message } => message,
                Ask::Relay { body } => body,
            };
            reused || !within(body, limits.call_bytes)
        }
        Up::Fact { fact } => !within(fact, limits.fact_bytes),
        Up::Long { span: _ } | Up::Waiting => false,
        Up::Finish { finish } => match finish {
            Finish::Ended { outcome } => !within(outcome, limits.outcome_bytes),
            Finish::Parked { snapshot } => !within_optional(snapshot.as_deref(), limits.snapshot_bytes),
            Finish::Failed { failure: _ } => false,
        },
    }
}

/// An answer as it may go down: a relayed answer beyond the limits goes as
/// too large.
fn bounded(reply: Reply, limits: &Limits) -> Reply {
    match reply {
        Reply::Relayed { answer } => {
            if within(&answer, limits.answer_bytes) {
                Reply::Relayed { answer }
            } else {
                Reply::TooLarge
            }
        }
        reply @ (Reply::Pushed(_) | Reply::Unavailable | Reply::Busy | Reply::TooLarge) => reply,
    }
}

fn within(bytes: &[u8], most: u64) -> bool {
    match u64::try_from(bytes.len()) {
        Ok(len) => len <= most,
        Err(_) => false,
    }
}

fn within_optional(bytes: Option<&[u8]>, most: u64) -> bool {
    match bytes {
        Some(bytes) => within(bytes, most),
        None => true,
    }
}

/// The last `Limits::detail_bytes` of `detail`.
fn tail(detail: Box<[u8]>, limits: &Limits) -> Box<[u8]> {
    let keep = usize::try_from(limits.detail_bytes).expect("a u32 fits in a usize");
    match detail.len().checked_sub(keep) {
        Some(cut) if cut > 0 => copy_of(detail.get(cut..).expect("cut within the detail")),
        Some(_) | None => detail,
    }
}
