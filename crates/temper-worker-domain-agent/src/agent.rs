//! Agent processes: one per hosted run, each with one channel over its pipes
//! (worker-domain.md, section 6).
//!
//! A `Spawn` takes a slot, or is refused at the entrance, and has io spawn
//! the agent in a contained process tree, within a deadline. Once it runs, its
//! run's start message goes down first and the run is live: inbound events
//! and the answers to its host calls go down as the client gives them, one
//! message in flight at a time and the rest waiting in order in the run's
//! outbox; what the run says comes up one message per read, checked against
//! the channel's rules (the `channel` module) as it comes. Host calls and
//! their withdrawals go to the client, and so do the run's facts, its waiting,
//! and how it finishes, after which anything more it says breaks the rules. A
//! call beyond the run's limit is answered as busy at the entrance, its answer
//! going down ahead of the outbox: one busy answer may wait while the run is
//! read on, and while a second waits behind it nothing more is read, so what
//! one message may cause always has room (programming-model.md, section 7).
//!
//! The watchdog runs while the run is live. Every message counts as progress,
//! and silence for `Limits::no_progress` while its clock runs fails the agent.
//! The clock pauses while the run waits for an inbound event, having read
//! every one sent down to it and with none waiting to go (from its `Waiting`
//! until the client delivers one, or the run calls or starts a long
//! operation), and while a call of its waits for the client's answer; it runs
//! afresh as it resumes. A long operation the run reports holds it until the
//! operation's deadline, or until the run says it is done.
//!
//! Stopping is cancel, then kill. The client's stop sends the cancel down,
//! behind what waits, and the run winds down on its own within
//! `Limits::grace`; so does the wall time, which is owed to the client as the
//! agent's fault if the run does not say how it finishes, or says only that
//! it was cancelled: that cancel was the agent's fault, not the run's own
//! ending. A run that says how it finishes is given the same grace to exit. Past the grace the tree is
//! terminated, and past `Limits::kill_after` after that, killed. A broken rule
//! or the watchdog terminates the tree at once, and so does a live run that
//! hangs up its channel without saying how it finishes. A process that exits,
//! or stops reading its channel, before its run has said how it finishes is
//! drained instead: what it wrote is read, a finish among it, until the
//! channel up ends or the grace is up; a drained channel that ends is waited
//! for within the rest of the grace, as the process has exited, or soon will.
//!
//! The client hears at most one of `Finished` and `Faulted`, and a fault only
//! while the run is live: once the client has stopped it, or the run has said
//! how it finishes, a fault still stops the agent, and only a fact tells of
//! it. A run that finishes after a stop is heard, and so is one that finishes
//! while it is being terminated or killed, as long as the client has heard
//! nothing yet: how it finishes is its own to say.
//!
//! An agent has gone, and the client is told so, only once its process has
//! exited and its tree is empty (io's `Exited` and `Reaped`), the channel up
//! has ended, and nothing it asked of io is in flight. Every state reads the
//! channel up to its end, so that the layers below can release it.
//!
//! A transition table, by state; what is not listed is unreachable by the
//! boundary's contract (one terminal per request, `Exited` before `Reaped`,
//! the client naming an agent only once it has started). "Owed" is the fault
//! told if the run does not say how it finishes (or, for the wall time, says
//! it was cancelled): none once the client has stopped the agent.
//!
//! ```text
//! state        event                          next          requests
//! -            spawn, no slot                 -             gone: busy
//!              spawn, beyond the limits       -             gone: invalid
//!              spawn                          Spawning      spawn
//! Spawning     spawned                        Live          started, wait, reap, send: start
//!              unspawned                      Closed        gone: unspawned
//! Live         received: a call               Live          called, or a busy answer down
//!              received: a withdraw           Live          withdrawn, or (dropped)
//!              received: a fact               Live          told
//!              received: long, long done      Live
//!              received: waiting              Live          waiting
//!              received: finish               Exiting       finished
//!              received: a broken rule        Terminating   faulted: rules, terminate
//!              malformed                      Terminating   faulted: rules, terminate
//!              hangup                         Terminating   faulted: exited, terminate
//!              exited, unsent                 Draining      (owed: exited)
//!              deliver                        Live          (the event down), or bounced:
//!                                                             too large, full
//!              answer                         Live          (the answer down)
//!              stop                           Cancelled     (the cancel down)
//!              watchdog                       Terminating   faulted: no progress, terminate
//!              wall time                      Cancelled     (the cancel down; owed: wall time)
//! Cancelled    received: a call               Cancelled     called, or a busy answer down
//!              received: a withdraw           Cancelled     withdrawn, or (dropped)
//!              received: a fact               Cancelled     told
//!              received: long, long done,     Cancelled
//!                waiting
//!              received: finish               Exiting       finished, or faulted: wall time if
//!                                                             owed and it says it was cancelled
//!              received: a broken rule        Terminating   faulted: rules if owed, terminate
//!              malformed                      Terminating   faulted: rules if owed, terminate
//!              hangup                         Exiting       faulted: what is owed
//!              exited, unsent                 Draining
//!              deliver                        Cancelled     bounced: ending
//!              answer                         Cancelled     (the answer down)
//!              stop                           Cancelled     (nothing owed)
//!              grace                          Terminating   faulted: what is owed, terminate
//! Draining     received: a fact               Draining      told
//!              received: a call, a withdraw,  Draining      (dropped: nothing hears its
//!                long, long done, waiting                     answer)
//!              received: finish               Exiting       finished, or faulted: as Cancelled
//!              received: a broken rule        Terminating   faulted: rules if owed, terminate
//!              malformed                      Terminating   faulted: rules if owed, terminate
//!              hangup                         Exiting       faulted: what is owed
//!              deliver                        Draining      bounced: ending
//!              answer                         Draining      (dropped)
//!              stop                           Draining      (nothing owed)
//!              grace                          Terminating   faulted: what is owed, terminate
//! Exiting      received, malformed            Terminating   terminate (it broke the rules)
//!              hangup, exited                 Exiting
//!              deliver                        Exiting       bounced: ending
//!              answer, stop                   Exiting       (dropped)
//!              grace                          Terminating   terminate
//! Terminating  received: finish, nothing told Terminating   finished
//!              received                       Terminating   (dropped)
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

use skein_lib::bytes::copy_of;
use skein_lib::{Deadlines, Env, Id, Queue, Set, Slab, Time, Token};

use crate::boundary::{Bounce, End, Fault, Invalid, Request, Signal, Spawn};
use crate::channel::{Ask, Down, Finish, Grant, Reply, RunFailure, Up};
use crate::domain::Domain;
use crate::facts::{Fact, Facts};
use crate::limits::{self, BUSY, Limits};

#[derive(Debug)]
pub(crate) struct Agent {
    /// The client's token, echoed on every record back to it.
    client: Token,
    state: State,
}

/// The agent child domain's alarms, each for one agent.
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
enum First {
    Legacy {
        charter: Box<[u8]>,
        snapshot: Option<Box<[u8]>>,
        repositories: Box<[crate::channel::Repository]>,
        grants: Box<[Grant]>,
    },
    V2 {
        spawn: crate::SpawnV2,
    },
}

#[derive(Debug)]
enum State {
    /// Its process is being spawned; what its run starts with waits for it.
    Spawning { start: First },
    /// Its run is live: the channel is open both ways, and the watchdog and
    /// the wall time run.
    Live { process: Process, channel: Channel, watch: Watch },
    /// The client stopped it, or its wall time ran out: the cancel goes down,
    /// and the run winds down on its own until `until`. `owed` is the fault
    /// the client is told if the run does not say how it finishes.
    Cancelled { process: Process, channel: Channel, until: Time, owed: Option<Fault> },
    /// Its process exited, or stopped reading its channel, before its run said
    /// how it finishes: what it wrote is read until `until`, and `owed` told
    /// if no finish comes.
    Draining { process: Process, until: Time, owed: Option<Fault> },
    /// Its run has nothing more to say: it said how it finishes, or its
    /// channel ended once it had left live. Its process goes on its own until
    /// `until`. `told` once the client has heard how the run finishes or how
    /// the agent failed.
    Exiting { process: Process, until: Time, told: bool },
    /// Its tree was told to exit, and is killed at `until`.
    Terminating { process: Process, until: Time, told: bool },
    /// Its tree was killed.
    Killing { process: Process, told: bool },
    /// Terminal: holds nothing.
    Closed,
}

/// What an agent asked of io for its process, and what io said of it.
#[derive(Debug)]
struct Process {
    name: Token,
    reading: Reading,
    version: Version,
    credit: bool,
    turns: u32,
    spent: u64,
    /// A send is in flight.
    sending: bool,
    signals: u32,
    exited: bool,
    /// Once its tree is empty, the detail of its end.
    reaped: Option<Box<[u8]>>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Version {
    Legacy,
    V2,
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
    events: u32,
    accounts: Set<u32>,
    /// Sent event names awaiting an acknowledgement, in delivery order.
    sent: Queue<Token>,
    acknowledged: Option<Token>,
    /// Calls answered as busy at the entrance: their answers go down ahead of
    /// the outbox, and while two wait, nothing more is read. Their names stay
    /// in flight here until the answers go down, separately from admitted
    /// calls in `flight`.
    busy: Queue<Token>,
    /// The names of the run's calls in flight, from the call until its answer
    /// is sent down.
    flight: Set<Token>,
    /// Those the client has not answered yet.
    asked: Set<Token>,
    /// Those of them the run withdrew.
    withdrawn: Set<Token>,
}

/// The watchdog's view of a live run.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Watch {
    /// When it last made progress, or its clock last resumed.
    seen: Time,
    /// Until when a long operation it reported may run.
    held: Time,
    /// It waits for its next inbound event, having read every one sent.
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

pub(crate) fn spawn(domain: &mut Domain, env: &Env<Limits>, client: Token, spawn: Spawn, out: &mut Queue<Request>) {
    let Spawn { workspace, charter, snapshot, repositories, grants } = spawn;
    let limits = &env.limits;
    let refusal = if domain.agents.is_full() {
        Some(End::Busy)
    } else if !within(&charter, limits.charter_bytes) {
        Some(End::Invalid(Invalid::Charter))
    } else if !within_optional(snapshot.as_deref(), limits.snapshot_bytes) {
        Some(End::Invalid(Invalid::Snapshot))
    } else if u64::try_from(repositories.len()).expect("a length fits") > u64::from(limits.repositories)
        || !repository_names(&repositories, limits.name_bytes)
    {
        Some(End::Invalid(Invalid::Repositories))
    } else if !grant_names(&grants, limits.accounts) {
        Some(End::Invalid(Invalid::Grants))
    } else {
        None
    };
    if let Some(end) = refusal {
        domain.facts.push(Fact::Gone { client, end });
        out.push(Request::Gone { client, end, detail: Box::new([]) });
        return;
    }
    let agent =
        Agent { client, state: State::Spawning { start: First::Legacy { charter, snapshot, repositories, grants } } };
    let id = domain.agents.insert(agent).expect("checked for room above");
    let deadline = env.now.saturating_add(limits.spawn_timeout);
    out.push(Request::Spawn { owner: id.token(), workspace, deadline });
}

pub(crate) fn deliver(
    domain: &mut Domain,
    env: &Env<Limits>,
    agent: Token,
    name: Token,
    event: Box<[u8]>,
    out: &mut Queue<Request>,
) {
    let Some(id) = addressed(&domain.agents, agent) else {
        return;
    };
    let entry = domain.agents.get_mut(id).expect("addressed above");
    let client = entry.client;
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Live { process, channel, watch } => delivered(client, process, channel, watch, name, event, env, out),
        // It no longer listens.
        state @ (State::Cancelled { .. }
        | State::Draining { .. }
        | State::Exiting { .. }
        | State::Terminating { .. }
        | State::Killing { .. }) => {
            out.push(Request::Bounced { client, name, bounce: Bounce::Ending });
            state
        }
        State::Spawning { .. } | State::Closed => unreachable!("an addressed agent has started and not gone"),
    };
    follow(domain, env, id, out);
}

pub(crate) fn answer(
    domain: &mut Domain,
    env: &Env<Limits>,
    agent: Token,
    call: Token,
    reply: Reply,
    out: &mut Queue<Request>,
) {
    let Some(id) = addressed(&domain.agents, agent) else {
        return;
    };
    let entry = domain.agents.get_mut(id).expect("addressed above");
    let reply = bounded(reply, &env.limits);
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Live { process, mut channel, watch } => {
            answered(&mut channel, call, reply);
            // The clock resumes once no call waits for the client.
            let watch = if channel.asked.is_empty() { Watch { seen: env.now, ..watch } } else { watch };
            State::Live { process, channel, watch }
        }
        State::Cancelled { process, mut channel, until, owed } => {
            answered(&mut channel, call, reply);
            State::Cancelled { process, channel, until, owed }
        }
        // It no longer listens: the answer is dropped.
        state
        @ (State::Draining { .. } | State::Exiting { .. } | State::Terminating { .. } | State::Killing { .. }) => state,
        State::Spawning { .. } | State::Closed => unreachable!("an addressed agent has started and not gone"),
    };
    follow(domain, env, id, out);
}

pub(crate) fn stop(domain: &mut Domain, env: &Env<Limits>, agent: Token, out: &mut Queue<Request>) {
    let Some(id) = addressed(&domain.agents, agent) else {
        return;
    };
    let Domain { agents, alarms: _, facts } = domain;
    let entry = agents.get_mut(id).expect("addressed above");
    let client = entry.client;
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Live { process, channel, watch: _ } => {
            facts.push(Fact::Cancelled { client });
            cancel(process, channel, None, env)
        }
        // The client has stopped it: nothing is owed to it any more.
        State::Cancelled { process, channel, until, owed: _ } => {
            State::Cancelled { process, channel, until, owed: None }
        }
        State::Draining { process, until, owed: _ } => State::Draining { process, until, owed: None },
        // It is stopping already.
        state @ (State::Exiting { .. } | State::Terminating { .. } | State::Killing { .. }) => state,
        State::Spawning { .. } | State::Closed => unreachable!("an addressed agent has started and not gone"),
    };
    follow(domain, env, id, out);
}

pub(crate) fn spawned(domain: &mut Domain, env: &Env<Limits>, owner: Token, process: Token, out: &mut Queue<Request>) {
    let Domain { agents, alarms: _, facts } = domain;
    let id = Id::<Agent>::from_token(owner);
    let entry = agents.get_mut(id).expect("an agent lives until its requests have ended");
    let names = Names { owner, client: entry.client };
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Spawning { start: first } => start(names, process, first, env, facts, out),
        State::Live { .. }
        | State::Cancelled { .. }
        | State::Draining { .. }
        | State::Exiting { .. }
        | State::Terminating { .. }
        | State::Killing { .. }
        | State::Closed => unreachable!("only a spawning agent's spawn ends"),
    };
    follow(domain, env, id, out);
}

pub(crate) fn unspawned(
    domain: &mut Domain,
    env: &Env<Limits>,
    owner: Token,
    detail: Box<[u8]>,
    out: &mut Queue<Request>,
) {
    let Domain { agents, alarms: _, facts } = domain;
    let id = Id::<Agent>::from_token(owner);
    let entry = agents.get_mut(id).expect("an agent lives until its requests have ended");
    let client = entry.client;
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Spawning { start: _ } => {
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
    follow(domain, env, id, out);
}

pub(crate) fn sent(domain: &mut Domain, env: &Env<Limits>, owner: Token, out: &mut Queue<Request>) {
    let id = Id::<Agent>::from_token(owner);
    let entry = domain.agents.get_mut(id).expect("an agent lives until its requests have ended");
    let process = process_of(&mut entry.state);
    assert!(process.sending, "a send ends once");
    process.sending = false;
    follow(domain, env, id, out);
}

pub(crate) fn unsent(domain: &mut Domain, env: &Env<Limits>, owner: Token, out: &mut Queue<Request>) {
    let id = Id::<Agent>::from_token(owner);
    let entry = domain.agents.get_mut(id).expect("an agent lives until its requests have ended");
    let process = process_of(&mut entry.state);
    assert!(process.sending, "a send ends once");
    process.sending = false;
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        // It no longer reads its channel: what it wrote is drained.
        State::Live { process, channel: _, watch: _ } => drain(process, env),
        State::Cancelled { process, channel: _, until, owed } => State::Draining { process, until, owed },
        state
        @ (State::Draining { .. } | State::Exiting { .. } | State::Terminating { .. } | State::Killing { .. }) => state,
        State::Spawning { .. } | State::Closed => unreachable!("only a spawned agent sends"),
    };
    follow(domain, env, id, out);
}

pub(crate) fn received(domain: &mut Domain, env: &Env<Limits>, owner: Token, message: Up, out: &mut Queue<Request>) {
    let Domain { agents, alarms: _, facts } = domain;
    let id = Id::<Agent>::from_token(owner);
    let entry = agents.get_mut(id).expect("an agent lives until its requests have ended");
    let names = Names { owner, client: entry.client };
    let process = process_of(&mut entry.state);
    assert!(process.reading == Reading::Asked, "a read ends once");
    process.reading = Reading::Idle;
    let wrong = !account(process, &message);
    let limits = &env.limits;
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Live { process, channel, watch } => {
            if wrong || broken(&message, Some(&channel), limits) {
                fail(names, process, Fault::Rules, true, env, facts, out)
            } else {
                heard(names.client, process, channel, watch, message, env, facts, out)
            }
        }
        State::Cancelled { process, channel, until, owed } => {
            if wrong || broken(&message, Some(&channel), limits) {
                fail(names, process, Fault::Rules, owed.is_some(), env, facts, out)
            } else {
                wound(names.client, process, channel, until, owed, message, facts, out)
            }
        }
        State::Draining { process, until, owed } => {
            if wrong || broken(&message, None, limits) {
                fail(names, process, Fault::Rules, owed.is_some(), env, facts, out)
            } else {
                drained(names.client, process, until, owed, message, facts, out)
            }
        }
        // Its run has said all it may: anything more breaks the rules.
        State::Exiting { process, until: _, told } => {
            tell_fault(names.client, Fault::Rules, false, facts, out);
            terminate(names, process, told, env, facts, out)
        }
        // What a tree being stopped writes is read to the end, and dropped,
        // but for a finish the client may still hear.
        State::Terminating { process, until, told } => {
            let told = if wrong { told } else { late(names.client, told, message, limits, facts, out) };
            State::Terminating { process, until, told }
        }
        State::Killing { process, told } => {
            let told = if wrong { told } else { late(names.client, told, message, limits, facts, out) };
            State::Killing { process, told }
        }
        State::Spawning { .. } | State::Closed => unreachable!("only a spawned agent reads"),
    };
    follow(domain, env, id, out);
}

pub(crate) fn malformed(domain: &mut Domain, env: &Env<Limits>, owner: Token, out: &mut Queue<Request>) {
    let Domain { agents, alarms: _, facts } = domain;
    let id = Id::<Agent>::from_token(owner);
    let entry = agents.get_mut(id).expect("an agent lives until its requests have ended");
    let names = Names { owner, client: entry.client };
    end_read(process_of(&mut entry.state));
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Live { process, .. } => fail(names, process, Fault::Rules, true, env, facts, out),
        State::Cancelled { process, owed, .. } | State::Draining { process, owed, .. } => {
            fail(names, process, Fault::Rules, owed.is_some(), env, facts, out)
        }
        State::Exiting { process, until: _, told } => {
            tell_fault(names.client, Fault::Rules, false, facts, out);
            terminate(names, process, told, env, facts, out)
        }
        state @ (State::Terminating { .. } | State::Killing { .. }) => state,
        State::Spawning { .. } | State::Closed => unreachable!("only a spawned agent reads"),
    };
    follow(domain, env, id, out);
}

pub(crate) fn hangup(domain: &mut Domain, env: &Env<Limits>, owner: Token, out: &mut Queue<Request>) {
    let Domain { agents, alarms: _, facts } = domain;
    let id = Id::<Agent>::from_token(owner);
    let entry = agents.get_mut(id).expect("an agent lives until its requests have ended");
    let names = Names { owner, client: entry.client };
    end_read(process_of(&mut entry.state));
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        // It stopped talking without saying how its run finishes.
        State::Live { process, .. } => fail(names, process, Fault::Exited, true, env, facts, out),
        // It has left live: it is waited for within the rest of its grace.
        State::Cancelled { process, until, owed, .. } | State::Draining { process, until, owed } => {
            owe(names.client, owed, facts, out);
            State::Exiting { process, until, told: owed.is_some() }
        }
        state @ (State::Exiting { .. } | State::Terminating { .. } | State::Killing { .. }) => state,
        State::Spawning { .. } | State::Closed => unreachable!("only a spawned agent reads"),
    };
    follow(domain, env, id, out);
}

pub(crate) fn signalled(domain: &mut Domain, env: &Env<Limits>, owner: Token, out: &mut Queue<Request>) {
    let id = Id::<Agent>::from_token(owner);
    let entry = domain.agents.get_mut(id).expect("an agent lives until its requests have ended");
    let process = process_of(&mut entry.state);
    process.signals = process.signals.checked_sub(1).expect("a signal ends once");
    follow(domain, env, id, out);
}

pub(crate) fn exited(domain: &mut Domain, env: &Env<Limits>, owner: Token, out: &mut Queue<Request>) {
    let id = Id::<Agent>::from_token(owner);
    let entry = domain.agents.get_mut(id).expect("an agent lives until its requests have ended");
    let process = process_of(&mut entry.state);
    assert!(!process.exited, "a wait ends once");
    process.exited = true;
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        // What it wrote before it went is drained.
        State::Live { process, channel: _, watch: _ } => drain(process, env),
        State::Cancelled { process, channel: _, until, owed } => State::Draining { process, until, owed },
        state
        @ (State::Draining { .. } | State::Exiting { .. } | State::Terminating { .. } | State::Killing { .. }) => state,
        State::Spawning { .. } | State::Closed => unreachable!("only a spawned agent is waited for"),
    };
    follow(domain, env, id, out);
}

pub(crate) fn reaped(
    domain: &mut Domain,
    env: &Env<Limits>,
    owner: Token,
    detail: Box<[u8]>,
    out: &mut Queue<Request>,
) {
    let id = Id::<Agent>::from_token(owner);
    let entry = domain.agents.get_mut(id).expect("an agent lives until its requests have ended");
    let process = process_of(&mut entry.state);
    assert!(process.exited, "io reports a process's exit before its reap");
    assert!(process.reaped.is_none(), "a reap ends once");
    process.reaped = Some(tail(detail, &env.limits));
    follow(domain, env, id, out);
}

pub(crate) fn watchdog(domain: &mut Domain, env: &Env<Limits>, id: Id<Agent>, out: &mut Queue<Request>) {
    let Domain { agents, alarms: _, facts } = domain;
    let entry = agents.get_mut(id).expect("an agent's alarms are cancelled before it is retired");
    let names = Names { owner: id.token(), client: entry.client };
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Live { process, .. } => fail(names, process, Fault::NoProgress, true, env, facts, out),
        State::Spawning { .. }
        | State::Cancelled { .. }
        | State::Draining { .. }
        | State::Exiting { .. }
        | State::Terminating { .. }
        | State::Killing { .. }
        | State::Closed => unreachable!("the watchdog runs only while the run is live"),
    };
    follow(domain, env, id, out);
}

pub(crate) fn wall(domain: &mut Domain, env: &Env<Limits>, id: Id<Agent>, out: &mut Queue<Request>) {
    let Domain { agents, alarms: _, facts } = domain;
    let entry = agents.get_mut(id).expect("an agent's alarms are cancelled before it is retired");
    let client = entry.client;
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        // The run is alive: it is cancelled, and may wind down and say how it
        // finishes.
        State::Live { process, channel, watch: _ } => {
            facts.push(Fact::Overdue { client });
            cancel(process, channel, Some(Fault::WallTime), env)
        }
        State::Spawning { .. }
        | State::Cancelled { .. }
        | State::Draining { .. }
        | State::Exiting { .. }
        | State::Terminating { .. }
        | State::Killing { .. }
        | State::Closed => unreachable!("the wall time runs only while the run is live"),
    };
    follow(domain, env, id, out);
}

pub(crate) fn grace(domain: &mut Domain, env: &Env<Limits>, id: Id<Agent>, out: &mut Queue<Request>) {
    let Domain { agents, alarms: _, facts } = domain;
    let entry = agents.get_mut(id).expect("an agent's alarms are cancelled before it is retired");
    let names = Names { owner: id.token(), client: entry.client };
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Cancelled { process, owed, .. } | State::Draining { process, owed, .. } => {
            owe(names.client, owed, facts, out);
            terminate(names, process, owed.is_some(), env, facts, out)
        }
        State::Exiting { process, until: _, told } => terminate(names, process, told, env, facts, out),
        State::Terminating { process, until: _, told } => kill(names, process, told, facts, out),
        State::Spawning { .. } | State::Live { .. } | State::Killing { .. } | State::Closed => {
            unreachable!("the grace runs only while an agent is stopping, until it is killed")
        }
    };
    follow(domain, env, id, out);
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
        | State::Killing { process, .. } => process,
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
fn follow(domain: &mut Domain, env: &Env<Limits>, id: Id<Agent>, out: &mut Queue<Request>) {
    let Domain { agents, alarms, facts } = domain;
    let entry = agents.get_mut(id).expect("an agent lives until it is retired");
    flow(id.token(), &mut entry.state, out);
    settle(entry, facts, out);
    let (watchdog, wall, grace) = timers(&entry.state, &env.limits);
    // Cancelled before the grace is armed: two alarms an agent at most.
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
/// message waiting goes down once none is in flight, busy answers first; and
/// in every state the next message up is read once none is being read, until
/// the channel up ends, but while two busy answers wait.
fn flow(owner: Token, state: &mut State, out: &mut Queue<Request>) {
    let (process, reads) = match state {
        State::Live { process, channel, .. } | State::Cancelled { process, channel, .. } => {
            send_next(owner, process, channel, out);
            let reads = channel.busy.len() < BUSY && process.credit;
            (process, reads)
        }
        State::Draining { process, .. } => {
            let reads = process.credit;
            (process, reads)
        }
        State::Exiting { process, .. } | State::Terminating { process, .. } | State::Killing { process, .. } => {
            (process, true)
        }
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
    let message = match channel.busy.pop() {
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

/// What leaves the outbox for the channel: an event no longer waits, and is
/// sent; an answer's call is no longer in flight.
fn leaving(channel: &mut Channel, message: &Down) {
    match message {
        Down::Event { name, .. } => {
            channel.events = channel.events.checked_sub(1).expect("the events waiting are counted");
            channel.sent.push(*name);
        }
        Down::Answer { call, reply: _ } => {
            let flying = channel.flight.remove(call);
            assert!(flying, "a call is in flight until its answer goes down");
        }
        Down::Cancel | Down::Grant { .. } => {}
        Down::Start { .. } | Down::StartV2 { .. } => {
            unreachable!("the start message goes down first, never from the outbox")
        }
    }
}

/// An agent whose run has nothing more to say has gone once its process has:
/// exited, its tree empty, the channel up ended, and nothing asked of io in
/// flight. The client is told, with the detail of its end.
fn settle(entry: &mut Agent, facts: &mut Facts, out: &mut Queue<Request>) {
    let settled = match &entry.state {
        State::Exiting { process, .. } | State::Terminating { process, .. } | State::Killing { process, .. } => {
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
        State::Exiting { process, .. } | State::Terminating { process, .. } | State::Killing { process, .. } => {
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
        State::Live { process, channel, watch } => {
            let runs = channel.asked.is_empty() && !watch.waiting && process.credit;
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
    first: First,
    env: &Env<Limits>,
    facts: &mut Facts,
    out: &mut Queue<Request>,
) -> State {
    let owner = names.owner;
    let limits = &env.limits;
    out.push(Request::Started { client: names.client, agent: owner });
    out.push(Request::Wait { owner, process });
    out.push(Request::Reap { owner, process });
    let mut accounts = Set::with_capacity(limits.accounts);
    let grant_names = match &first {
        First::V2 { spawn } => &spawn.grants,
        First::Legacy { grants, .. } => grants,
    };
    for grant in grant_names {
        accounts.insert(grant.account).expect("validated grant accounts");
    }
    let (version, message) = match first {
        First::V2 { spawn } => (
            Version::V2,
            Down::StartV2 {
                charter: spawn.charter,
                transcript: spawn.transcript,
                repositories: spawn.repositories,
                grants: spawn.grants,
            },
        ),
        First::Legacy { charter, snapshot, repositories, grants } => {
            (Version::Legacy, Down::Start { charter, snapshot, repositories, grants })
        }
    };
    out.push(Request::Send { owner, process, message });
    facts.push(Fact::Started { client: names.client });
    let outbox = limits::outbox(limits).expect("worst_case accepted the limits");
    let process = Process {
        name: process,
        reading: Reading::Idle,
        version,
        credit: true,
        turns: 0,
        spent: 0,
        sending: true,
        signals: 0,
        exited: false,
        reaped: None,
    };
    let channel = Channel {
        outbox: Queue::with_capacity(outbox),
        events: 0,
        accounts,
        sent: Queue::with_capacity(limits.events),
        acknowledged: None,
        busy: Queue::with_capacity(BUSY),
        flight: Set::with_capacity(limits.calls),
        asked: Set::with_capacity(limits.calls),
        withdrawn: Set::with_capacity(limits.calls),
    };
    let now = env.now;
    let watch = Watch { seen: now, held: now, waiting: false, wall: now.saturating_add(limits.wall_time) };
    State::Live { process, channel, watch }
}

/// Live, deliver: the event waits to go down, or is bounced; a run that
/// waited for it resumes its clock.
#[expect(clippy::too_many_arguments, reason = "a cell handler takes the fields it touches")]
fn delivered(
    client: Token,
    process: Process,
    mut channel: Channel,
    watch: Watch,
    name: Token,
    event: Box<[u8]>,
    env: &Env<Limits>,
    out: &mut Queue<Request>,
) -> State {
    let limits = &env.limits;
    if !within(&event, limits.event_bytes) {
        out.push(Request::Bounced { client, name, bounce: Bounce::TooLarge });
        return State::Live { process, channel, watch };
    }
    if channel.events.saturating_add(channel.sent.len()) >= limits.events {
        out.push(Request::Bounced { client, name, bounce: Bounce::Full });
        return State::Live { process, channel, watch };
    }
    channel.outbox.push(Down::Event { name, event });
    channel.events = channel.events.checked_add(1).expect("no more events wait than the limits allow");
    let watch = if watch.waiting { Watch { seen: env.now, waiting: false, ..watch } } else { watch };
    State::Live { process, channel, watch }
}

/// The client's answer to a call of the run waits to go down.
fn answered(channel: &mut Channel, call: Token, reply: Reply) {
    let asked = channel.asked.remove(&call);
    assert!(asked, "the client answers each call it was told of, once");
    channel.withdrawn.remove(&call);
    channel.outbox.push(Down::Answer { call, reply });
}

/// Live, stop or wall time: the cancel goes down behind what waits, and
/// `owed` is told if the run does not say how it finishes.
fn cancel(process: Process, mut channel: Channel, owed: Option<Fault>, env: &Env<Limits>) -> State {
    channel.outbox.push(Down::Cancel);
    State::Cancelled { process, channel, until: env.now.saturating_add(env.limits.grace), owed }
}

/// Live, exited or unsent: what it wrote is drained for the grace, and the
/// exit is owed if no finish comes.
fn drain(process: Process, env: &Env<Limits>) -> State {
    State::Draining { process, until: env.now.saturating_add(env.limits.grace), owed: Some(Fault::Exited) }
}

/// Live, received a message within the rules: whatever it is, it is progress.
#[expect(clippy::too_many_arguments, reason = "a cell handler takes the fields it touches")]
fn heard(
    client: Token,
    mut process: Process,
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
        Up::Turn { turn } => {
            if let Some(read) = turn.read {
                acknowledge(&mut channel, read);
            }
            process.credit = false;
            out.push(Request::Turn { client, turn });
        }
        Up::FinishV2 { turns, spent, finish } => {
            return finished_v2(
                client,
                process,
                turns,
                spent,
                finish,
                now.saturating_add(env.limits.grace),
                None,
                facts,
                out,
            );
        }
        Up::Call { call, ask } => {
            watch.waiting = false;
            called(client, &mut channel, call, ask, out);
        }
        Up::Withdraw { call } => withdrew(client, &mut channel, call, out),
        Up::Fact { fact } => out.push(Request::Told { client, fact }),
        Up::Rejected { account, generation } => out.push(Request::Rejected { client, account, generation }),
        Up::Exhausted { account, retry_after } => out.push(Request::Exhausted { client, account, retry_after }),
        Up::Long { span } => {
            watch.waiting = false;
            watch.held = watch.held.max(now.saturating_add(span));
        }
        Up::LongDone => watch.held = now,
        Up::Waiting { heard } => {
            // A wait that crossed an event on its way is no wait: the run is
            // about to read it.
            acknowledge(&mut channel, Token::new(heard));
            watch.waiting = channel.sent.is_empty() && channel.events == 0;
            out.push(Request::Waiting { client });
        }
        Up::Finish { finish } => {
            return finished(client, process, finish, now.saturating_add(env.limits.grace), None, facts, out);
        }
    }
    State::Live { process, channel, watch }
}

/// Cancelled, received a message within the rules: the run winds down, and
/// the watchdog no longer runs.
#[expect(clippy::too_many_arguments, reason = "a cell handler takes the fields it touches")]
fn wound(
    client: Token,
    mut process: Process,
    mut channel: Channel,
    until: Time,
    owed: Option<Fault>,
    message: Up,
    facts: &mut Facts,
    out: &mut Queue<Request>,
) -> State {
    match message {
        Up::Turn { turn } => {
            if let Some(read) = turn.read {
                acknowledge(&mut channel, read);
            }
            process.credit = false;
            out.push(Request::Turn { client, turn });
        }
        Up::FinishV2 { turns, spent, finish } => {
            return finished_v2(client, process, turns, spent, finish, until, owed, facts, out);
        }
        Up::Call { call, ask } => called(client, &mut channel, call, ask, out),
        Up::Withdraw { call } => withdrew(client, &mut channel, call, out),
        Up::Fact { fact } => out.push(Request::Told { client, fact }),
        Up::Rejected { account, generation } => out.push(Request::Rejected { client, account, generation }),
        Up::Exhausted { account, retry_after } => out.push(Request::Exhausted { client, account, retry_after }),
        Up::Long { span: _ } | Up::LongDone | Up::Waiting { heard: _ } => {}
        Up::Finish { finish } => return finished(client, process, finish, until, owed, facts, out),
    }
    State::Cancelled { process, channel, until, owed }
}

/// Draining, received a message within the rules: what the run wrote before
/// it went. Nothing would hear an answer to a call, nor of its withdrawal.
fn drained(
    client: Token,
    mut process: Process,
    until: Time,
    owed: Option<Fault>,
    message: Up,
    facts: &mut Facts,
    out: &mut Queue<Request>,
) -> State {
    match message {
        Up::Turn { turn } => {
            process.credit = false;
            out.push(Request::Turn { client, turn });
        }
        Up::FinishV2 { turns, spent, finish } => {
            return finished_v2(client, process, turns, spent, finish, until, owed, facts, out);
        }
        Up::Call { .. } | Up::Withdraw { .. } | Up::Long { .. } | Up::LongDone | Up::Waiting { .. } => {}
        Up::Fact { fact } => out.push(Request::Told { client, fact }),
        Up::Rejected { account, generation } => out.push(Request::Rejected { client, account, generation }),
        Up::Exhausted { account, retry_after } => out.push(Request::Exhausted { client, account, retry_after }),
        Up::Finish { finish } => return finished(client, process, finish, until, owed, facts, out),
    }
    State::Draining { process, until, owed }
}

/// Terminating or Killing, received: dropped, but for a finish within the
/// limits that the client may still hear, having heard neither how the run
/// finishes nor how the agent failed. Whether it has heard one now.
fn late(client: Token, told: bool, message: Up, limits: &Limits, facts: &mut Facts, out: &mut Queue<Request>) -> bool {
    if told || broken(&message, None, limits) {
        return told;
    }
    match message {
        Up::FinishV2 { turns, spent, finish } => {
            facts.push(Fact::Finished { client });
            out.push(Request::FinishedV2 { client, turns, spent, finish });
            true
        }
        Up::Finish { finish } => {
            facts.push(Fact::Finished { client });
            out.push(Request::Finished { client, finish });
            true
        }
        Up::Turn { .. }
        | Up::Call { .. }
        | Up::Withdraw { .. }
        | Up::Fact { .. }
        | Up::Long { .. }
        | Up::LongDone
        | Up::Waiting { .. }
        | Up::Rejected { .. }
        | Up::Exhausted { .. } => told,
    }
}

/// A host call within the rules goes to the client, or, if the run has as
/// many in flight as it may, is answered as busy.
fn called(client: Token, channel: &mut Channel, call: Token, ask: Ask, out: &mut Queue<Request>) {
    match channel.flight.insert(call) {
        Ok(fresh) => {
            assert!(fresh, "a call reusing a name in flight breaks the rules");
            let asked = channel.asked.insert(call);
            assert!(asked == Ok(true), "the calls asked are among those in flight");
            out.push(Request::Called { client, call, ask });
        }
        Err(call) => channel.busy.push(call),
    }
}

/// A withdraw within the rules: the client hears of it while it has the call
/// to answer. Otherwise the call's answer is on its way down, or crossed the
/// withdraw, and there is nothing to tell.
fn withdrew(client: Token, channel: &mut Channel, call: Token, out: &mut Queue<Request>) {
    if !channel.asked.contains(&call) {
        return;
    }
    let fresh = channel.withdrawn.insert(call);
    assert!(fresh == Ok(true), "a call withdrawn twice breaks the rules; the withdrawn are among those asked");
    out.push(Request::Withdrawn { client, call });
}

/// The run said how it finishes: its last word. Its process exits on its own
/// until `until`. A run the wall time cancelled, `owed` the client, that says
/// it was cancelled has not said how it finishes of its own: the client is
/// told the fault instead.
fn finished(
    client: Token,
    process: Process,
    finish: Finish,
    until: Time,
    owed: Option<Fault>,
    facts: &mut Facts,
    out: &mut Queue<Request>,
) -> State {
    let cancelled = finish == Finish::Failed { failure: RunFailure::Cancelled };
    match owed {
        Some(Fault::WallTime) if cancelled => tell_fault(client, Fault::WallTime, true, facts, out),
        Some(Fault::WallTime | Fault::Exited | Fault::Rules | Fault::NoProgress) | None => {
            facts.push(Fact::Finished { client });
            out.push(Request::Finished { client, finish });
        }
    }
    State::Exiting { process, until, told: true }
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
    tell_fault(names.client, fault, told, facts, out);
    terminate(names, process, told, env, facts, out)
}

/// The fault the client is owed, if it is: told now.
fn owe(client: Token, owed: Option<Fault>, facts: &mut Facts, out: &mut Queue<Request>) {
    if let Some(fault) = owed {
        tell_fault(client, fault, true, facts, out);
    }
}

fn tell_fault(client: Token, fault: Fault, told: bool, facts: &mut Facts, out: &mut Queue<Request>) {
    facts.push(Fact::Faulted { client, fault });
    if told {
        out.push(Request::Faulted { client, fault });
    }
}

/// Its tree is told to exit, and killed past `Limits::kill_after`.
fn terminate(
    names: Names,
    mut process: Process,
    told: bool,
    env: &Env<Limits>,
    facts: &mut Facts,
    out: &mut Queue<Request>,
) -> State {
    signal(names.owner, &mut process, Signal::Terminate, out);
    facts.push(Fact::Terminated { client: names.client });
    State::Terminating { process, until: env.now.saturating_add(env.limits.kill_after), told }
}

/// Terminating, grace: its tree is killed.
fn kill(names: Names, mut process: Process, told: bool, facts: &mut Facts, out: &mut Queue<Request>) -> State {
    signal(names.owner, &mut process, Signal::Kill, out);
    facts.push(Fact::Killed { client: names.client });
    State::Killing { process, told }
}

fn signal(owner: Token, process: &mut Process, signal: Signal, out: &mut Queue<Request>) {
    out.push(Request::Signal { owner, process: process.name, signal });
    process.signals = process.signals.checked_add(1).expect("a tree is signalled twice at most");
}

/// Whether a message breaks the channel's rules: a payload or a span beyond
/// the limits, or, while the run listens on `channel`, a call reusing a name
/// in flight, a call withdrawn twice before its answer, or a wait that claims
/// an event name that was never sent down.
fn broken(message: &Up, channel: Option<&Channel>, limits: &Limits) -> bool {
    match message {
        Up::Turn { turn } => {
            let unknown = match turn.read {
                Some(read) => match channel {
                    Some(channel) => !known(channel, read),
                    None => false,
                },
                None => false,
            };
            unknown || !within(&turn.body, limits.turn_bytes)
        }
        Up::FinishV2 { finish, .. } => match finish {
            crate::channel::FinishV2::Ended { outcome } => !within(outcome, limits.outcome_bytes),
            crate::channel::FinishV2::Parked | crate::channel::FinishV2::Failed { .. } => false,
        },
        Up::Call { call, ask } => {
            let reused = match channel {
                Some(channel) => in_flight(channel, *call),
                None => false,
            };
            let too_large = match ask {
                Ask::Push { message } => !within(message, limits.call_bytes),
                Ask::Relay { body } => !within(body, limits.call_bytes),
                Ask::PushV2 { title, body } => match len(title).checked_add(len(body)) {
                    Some(bytes) => bytes > limits.call_bytes,
                    None => true,
                },
            };
            reused || too_large
        }
        Up::Withdraw { call } => match channel {
            Some(channel) => channel.withdrawn.contains(call),
            None => false,
        },
        Up::Fact { fact } => !within(fact, limits.fact_bytes),
        Up::Long { span } => *span > limits.long_span,
        Up::LongDone | Up::Rejected { .. } | Up::Exhausted { .. } => false,
        Up::Waiting { heard } => match channel {
            Some(channel) => !known(channel, Token::new(*heard)),
            None => false,
        },
        Up::Finish { finish } => match finish {
            Finish::Ended { outcome } => !within(outcome, limits.outcome_bytes),
            Finish::Parked { snapshot } => !within_optional(snapshot.as_deref(), limits.snapshot_bytes),
            Finish::Failed { failure: _ } => false,
        },
    }
}

/// Admitted calls and overflow calls both keep their names until their
/// answers go down. Overflow names already live in the bounded Busy queue.
fn in_flight(channel: &Channel, call: Token) -> bool {
    if channel.flight.contains(&call) {
        return true;
    }
    for waiting in &channel.busy {
        if *waiting == call {
            return true;
        }
    }
    false
}

/// An answer as it may go down: a relayed answer beyond the limits goes as
/// too large. Git diagnostics retain only the allowed tail, with dropped
/// bytes counted; typed push reasons always fit independently of byte payload.
fn bounded(reply: Reply, limits: &Limits) -> Reply {
    match reply {
        Reply::Relayed { answer } => {
            if within(&answer, limits.answer_bytes) {
                Reply::Relayed { answer }
            } else {
                Reply::TooLarge
            }
        }
        Reply::Pushed(crate::channel::Push::Conflicted { repository, files }) => {
            if repository >= limits.repositories || !paths(&files, limits) {
                Reply::TooLarge
            } else {
                Reply::Pushed(crate::channel::Push::Conflicted { repository, files })
            }
        }
        Reply::Pushed(crate::channel::Push::Failed { mut failure }) => {
            let output = failure.diagnostic.output();
            let keep = usize::try_from(limits.answer_bytes).unwrap_or(usize::MAX).min(output.len());
            let dropped = output.len().checked_sub(keep).expect("the tail is within the output");
            let cut =
                failure.diagnostic.cut().saturating_add(u64::try_from(dropped).expect("the fixed tail fits in u64"));
            failure.diagnostic =
                crate::PushDiagnostic::new(output.get(dropped..).expect("the tail is within the output"), cut);
            Reply::Pushed(crate::channel::Push::Failed { failure })
        }
        reply @ (Reply::Pushed(
            crate::channel::Push::Done | crate::channel::Push::Moved | crate::channel::Push::Nothing,
        )
        | Reply::Unavailable
        | Reply::Busy
        | Reply::Withdrawn
        | Reply::TooLarge) => reply,
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

/// Names are opaque. Their bounded delivery queue defines acknowledgement order.
fn known(channel: &Channel, heard: Token) -> bool {
    if channel.acknowledged == Some(heard) || (heard.raw() == 0 && channel.acknowledged.is_none()) {
        return true;
    }
    for name in &channel.sent {
        if *name == heard {
            return true;
        }
    }
    false
}

fn acknowledge(channel: &mut Channel, heard: Token) {
    if channel.acknowledged == Some(heard) || heard.raw() == 0 {
        return;
    }
    let count = channel.sent.len();
    for _ in 0..count {
        let name = channel.sent.pop().expect("an acknowledgement names a sent event");
        channel.acknowledged = Some(name);
        if name == heard {
            return;
        }
    }
    unreachable!("validated acknowledgement names a sent event");
}

fn repository_names(repositories: &[crate::channel::Repository], bytes: u32) -> bool {
    for repository in repositories {
        if repository.name.is_empty() || !within(&repository.name, u64::from(bytes)) {
            return false;
        }
    }
    true
}

fn grant_names(grants: &[Grant], accounts: u32) -> bool {
    if u64::try_from(grants.len()).expect("a length fits") > u64::from(accounts) {
        return false;
    }
    for (index, grant) in grants.iter().enumerate() {
        for other in grants.iter().skip(index.saturating_add(1)) {
            if grant.account == other.account {
                return false;
            }
        }
    }
    true
}

/// Refreshes replace a queued refresh of the same account, so the queue is bounded.
pub(crate) fn grant(domain: &mut Domain, env: &Env<Limits>, agent: Token, grant: Grant, out: &mut Queue<Request>) {
    let Some(id) = addressed(&domain.agents, agent) else {
        return;
    };
    let entry = domain.agents.get_mut(id).expect("addressed above");
    let channel = match &mut entry.state {
        State::Live { channel, .. } | State::Cancelled { channel, .. } => channel,
        State::Spawning { .. }
        | State::Draining { .. }
        | State::Exiting { .. }
        | State::Terminating { .. }
        | State::Killing { .. }
        | State::Closed => return,
    };
    if !channel.accounts.contains(&grant.account) {
        return;
    }
    let count = channel.outbox.len();
    let mut replaced = false;
    for _ in 0..count {
        let old = channel.outbox.pop().expect("the queue length is stable");
        let next = match old {
            Down::Grant { grant: old } if old.account == grant.account => {
                replaced = true;
                Down::Grant { grant }
            }
            old @ (Down::Start { .. }
            | Down::Event { .. }
            | Down::Answer { .. }
            | Down::Cancel
            | Down::Grant { .. }
            | Down::StartV2 { .. }) => old,
        };
        channel.outbox.push(next);
    }
    if !replaced {
        channel.outbox.push(Down::Grant { grant });
    }
    follow(domain, env, id, out);
}

pub(crate) fn spawn_v2(
    domain: &mut Domain,
    env: &Env<Limits>,
    client: Token,
    spawn: crate::SpawnV2,
    out: &mut Queue<Request>,
) {
    let limits = &env.limits;
    let refusal = if domain.agents.is_full() {
        Some(End::Busy)
    } else if !within(&spawn.charter, limits.charter_bytes) {
        Some(End::Invalid(Invalid::Charter))
    } else if !within_optional(spawn.transcript.as_deref(), limits.transcript_bytes) {
        Some(End::Invalid(Invalid::Transcript))
    } else if !repositories_v2(&spawn.repositories, limits) {
        Some(End::Invalid(Invalid::Repositories))
    } else if !grant_names(&spawn.grants, limits.accounts) {
        Some(End::Invalid(Invalid::Grants))
    } else {
        None
    };
    if let Some(end) = refusal {
        domain.facts.push(Fact::Gone { client, end });
        out.push(Request::Gone { client, end, detail: Box::new([]) });
        return;
    }
    let workspace = spawn.workspace;
    let first = First::V2 { spawn };
    let id = domain
        .agents
        .insert(Agent { client, state: State::Spawning { start: first } })
        .expect("checked for room above");
    out.push(Request::Spawn { owner: id.token(), workspace, deadline: env.now.saturating_add(limits.spawn_timeout) });
}

fn repositories_v2(repositories: &[crate::channel::RepositoryV2], limits: &Limits) -> bool {
    if len_count(repositories.len()) > u64::from(limits.repositories) {
        return false;
    }
    for (index, repository) in repositories.iter().enumerate() {
        if repository.name.is_empty()
            || !within(&repository.name, u64::from(limits.name_bytes))
            || len_count(repository.conflicts.len()) > u64::from(limits.conflicts)
        {
            return false;
        }
        for other in repositories.iter().skip(index.saturating_add(1)) {
            if repository.name == other.name {
                return false;
            }
        }
        for file in &repository.conflicts {
            if file.is_empty() || !within(file, u64::from(limits.path_bytes)) {
                return false;
            }
        }
    }
    true
}

fn len_count(value: usize) -> u64 {
    u64::try_from(value).expect("a length fits in u64")
}
fn len(bytes: &[u8]) -> u64 {
    len_count(bytes.len())
}

/// Validate mode and cumulative accounting before a turn leaves the child.
fn account(process: &mut Process, message: &Up) -> bool {
    match message {
        Up::Turn { turn } => {
            if process.version != Version::V2
                || process.turns.checked_add(1) != Some(turn.turn)
                || turn.spent < process.spent
            {
                false
            } else {
                process.turns = turn.turn;
                process.spent = turn.spent;
                true
            }
        }
        Up::FinishV2 { turns, spent, .. } => {
            process.version == Version::V2 && *turns == process.turns && *spent >= process.spent
        }
        Up::Finish { .. } => process.version != Version::V2,
        Up::Call { ask, .. } => match ask {
            Ask::PushV2 { .. } => process.version == Version::V2,
            Ask::Push { .. } => process.version != Version::V2,
            Ask::Relay { .. } => true,
        },
        Up::Withdraw { .. }
        | Up::Fact { .. }
        | Up::Long { .. }
        | Up::LongDone
        | Up::Waiting { .. }
        | Up::Rejected { .. }
        | Up::Exhausted { .. } => true,
    }
}

pub(crate) fn turn_credit(domain: &mut Domain, env: &Env<Limits>, agent: Token, read: bool, out: &mut Queue<Request>) {
    let Some(id) = addressed(&domain.agents, agent) else {
        return;
    };
    let entry = domain.agents.get_mut(id).expect("an addressed agent lives");
    match &mut entry.state {
        State::Live { process, watch, .. } => {
            process.credit = read;
            if read {
                watch.seen = env.now;
            }
        }
        State::Cancelled { process, .. } | State::Draining { process, .. } => process.credit = read,
        State::Spawning { .. }
        | State::Exiting { .. }
        | State::Terminating { .. }
        | State::Killing { .. }
        | State::Closed => return,
    }
    follow(domain, env, id, out);
}

#[expect(clippy::too_many_arguments, reason = "a terminal takes explicit cumulative accounting")]
fn finished_v2(
    client: Token,
    process: Process,
    turns: u32,
    spent: u64,
    finish: crate::channel::FinishV2,
    until: Time,
    owed: Option<Fault>,
    facts: &mut Facts,
    out: &mut Queue<Request>,
) -> State {
    let cancelled = finish == crate::channel::FinishV2::Failed { failure: RunFailure::Cancelled };
    match owed {
        Some(Fault::WallTime) if cancelled => tell_fault(client, Fault::WallTime, true, facts, out),
        Some(Fault::WallTime | Fault::Exited | Fault::Rules | Fault::NoProgress) | None => {
            facts.push(Fact::Finished { client });
            out.push(Request::FinishedV2 { client, turns, spent, finish });
        }
    }
    State::Exiting { process, until, told: true }
}

fn paths(files: &[Box<[u8]>], limits: &Limits) -> bool {
    if len_count(files.len()) > u64::from(limits.conflicts) {
        return false;
    }
    for file in files {
        if file.is_empty() || !within(file, u64::from(limits.path_bytes)) {
            return false;
        }
    }
    true
}
