//! Hosted runs: one run on the worker, from its assignment to its one answer
//! (worker-model.md, 4.2).
//!
//! An `Assign` admits a run to a slot, or refuses it at the entrance. An
//! admitted run has its workspace prepared, then its agent started on the
//! charter, resumed from the snapshot if there is one. While it is live,
//! inbound events go down to it as they arrive, and its host calls come up: a
//! push is served through the workspace, a forge read or an outlet is relayed
//! to the engine and its answer routed back. It parks or ends as it decides,
//! or fails; then the tail: its agent is stopped, its unfinished work saved if
//! the assignment asks and nothing of it landed, its workspace released, and
//! the engine answered, which frees the slot. A cancel (from the engine, or
//! from the top level for every run: lost contact, shutdown) or a fault of the
//! agent takes the same tail. The first ending decided wins.
//!
//! A run's transition table:
//!
//! ```text
//! state       event                         next        emits
//! -           assign, beyond the limits     -           answer: invalid
//!             assign, no slot, or hosted    -           answer: busy
//!             assign                        Preparing   prepare
//! Preparing   prepared                      Starting    start
//!             unprepared                    Closed      answer: unprepared
//!             inbound                       Preparing   (held), or bounced: full
//!             cancel                        Cancelling
//! Cancelling  prepared                      Closed      release, answer: cancelled
//!             unprepared                    Closed      answer: cancelled
//!             inbound                       Cancelling  bounced: ending
//!             cancel                        Cancelling
//! Starting    started                       Active      deliver what is held
//!             gone                          Closed      release, answer: unstarted
//!             inbound                       Starting    (held), or bounced: full
//!             cancel                        Unwanted
//! Unwanted    started                       Stopping    stop
//!             gone                          Closed      release, answer: cancelled
//!             inbound                       Unwanted    bounced: ending
//!             cancel                        Unwanted
//! Active      inbound                       Active      deliver
//!             called                        Active      push, relay, or reply: busy
//!             yielded                       Waiting
//!             finished                      Stopping    replies: unavailable, stop
//!             faulted                       Stopping    replies: unavailable, stop
//!             cancel                        Stopping    replies: unavailable, stop
//!             gone                          Stopping    replies: unavailable (exited)
//! Waiting     inbound                       Active      deliver
//!             called                        Active      push, relay, or reply: busy
//!             yielded                       Waiting
//!             finished, faulted, cancel     Stopping    as Active
//!             gone                          Stopping    as Active
//! Stopping    called                        Stopping    reply: unavailable
//!             yielded, finished, faulted    Stopping
//!             gone                          Stopping
//!             inbound                       Stopping    bounced: ending
//!             cancel                        Stopping
//!             settled, saving               Saving      save
//!             settled                       Closed      release, answer
//! Saving      saved                         Closed      release, answer
//!             inbound                       Saving      bounced: ending
//!             cancel                        Saving
//! ```
//!
//! A pushed or relayed event moves its call (the `call` module), and leaves
//! the run where it is. A stopping run has settled once its agent has gone
//! and no push of its is in flight; it then saves if the assignment asks and
//! none of its pushes landed, and otherwise releases its workspace and
//! answers. That follows from the state, in one place ([`conclude`]), which
//! also retires a run once it is Closed.
//!
//! Every other cell is unreachable by the contracts: the workspace's (one
//! terminal per request) and the agent's (`Started` first unless the agent
//! could not be started, then what its run does, then one `Gone`). Nothing
//! the engine sends reaches a run whose attempt it does not name, nor a
//! Closed run, which leaves the names as it closes.
//!
//! Inbound events that come while the run is not live yet are held, in the
//! order they came, up to the limit, and delivered as its agent starts; past
//! the limit they are bounced, and the engine keeps them. A run that never
//! goes live drops what it held: its answer says it took nothing.

use alloc::boxed::Box;
use core::mem;

use temper_lib::bytes::copy_of;
use temper_lib::{Env, Id, List, Map, Queue, ReplyTo, Set, Slab, Token};

use crate::assignment::{self, len};
use crate::boundary::{
    AgentFailure, Answer, Ask, Assignment, Bounce, Failure, Finish, Hosting, Landing, Phase, Preparation, Push, Reason,
    Refusal, Reply, Request, Work,
};
use crate::call::{self, Call};
use crate::facts::{Fact, Facts};
use crate::limits::Limits;
use crate::model::Model;

#[derive(Debug)]
pub(crate) struct Hosted {
    /// The engine's names for the run and for the attempt hosted.
    run: Token,
    attempt: Token,
    /// How many repositories its workspace lists.
    repositories: u32,
    /// The saved-work branch, if its unfinished work is saved; taken as it is.
    save: Option<Box<[u8]>>,
    /// The repositories its pushes landed in, by their place in the workspace.
    landed: Set<u32>,
    /// Its host calls in flight; once it has left live, the pushes still
    /// settling.
    calls: Set<Id<Call>>,
    state: State,
}

#[derive(Debug)]
enum State {
    /// Its workspace is being prepared. The charter and the snapshot wait for
    /// the agent, and inbound events in `held`.
    Preparing { reply_to: ReplyTo, charter: Box<[u8]>, snapshot: Option<Box<[u8]>>, held: Queue<Box<[u8]>> },
    /// Cancelled for `reason` as it was prepared: it answers once the prepare
    /// has settled.
    Cancelling { reply_to: ReplyTo, reason: Reason },
    /// Its agent is starting in `workspace`.
    Starting { reply_to: ReplyTo, workspace: Token, held: Queue<Box<[u8]>> },
    /// Cancelled for `reason` as its agent started: the agent is stopped once
    /// it has started.
    Unwanted { reply_to: ReplyTo, workspace: Token, reason: Reason },
    /// Its agent `agent` is at work.
    Active { reply_to: ReplyTo, workspace: Token, agent: Token },
    /// It yielded, and waits for its next inbound event.
    Waiting { reply_to: ReplyTo, workspace: Token, agent: Token },
    /// It ends with `ending` once its agent has gone (`gone`) and its pushes
    /// in flight have settled.
    Stopping { reply_to: ReplyTo, workspace: Token, agent: Token, ending: Ending, gone: bool },
    /// Its unfinished work is being saved; it ends with `ending` once it is.
    Saving { reply_to: ReplyTo, workspace: Token, ending: Ending },
    /// Terminal: holds nothing.
    Closed,
}

/// How a run ends.
#[derive(Debug)]
enum Ending {
    Ended { outcome: Box<[u8]> },
    Parked { snapshot: Option<Box<[u8]>> },
    Failed { failure: Failure, detail: Box<[u8]> },
}

// Entry points, one per event: look the run up, take its state out, run the
// cell's handler, conclude from the run's new state.

pub(crate) fn assign(
    model: &mut Model,
    env: &Env<Limits>,
    reply_to: ReplyTo,
    assignment: Assignment,
    out: &mut Queue<Request>,
) {
    let Model { hosted, names, calls: _, ready: _, facts } = model;
    // An assignment that can never fit is invalid, room or not: busy invites a
    // retry.
    if let Err(invalid) = assignment::check(&assignment, &env.limits) {
        refuse(reply_to, &assignment, Refusal::Invalid(invalid), out);
        return;
    }
    if hosted.is_full() || names.contains_key(&assignment.run) {
        refuse(reply_to, &assignment, Refusal::Busy, out);
        return;
    }
    let Assignment { run, attempt, workspace, save, charter, snapshot } = assignment;
    let repositories = u32::try_from(workspace.repositories.len()).expect("checked against the limits");
    let held = Queue::with_capacity(env.limits.held);
    let entry = Hosted {
        run,
        attempt,
        repositories,
        save,
        landed: Set::with_capacity(repositories),
        calls: Set::with_capacity(env.limits.run_calls),
        state: State::Preparing { reply_to, charter, snapshot, held },
    };
    let id = hosted.insert(entry).expect("checked for room above");
    let named = names.insert(run, id).expect("a name for every slot");
    assert!(named.is_none(), "checked the run is not hosted above");
    facts.push(Fact::Admitted { run, attempt });
    out.push(Request::Prepare { owner: id.token(), workspace });
}

pub(crate) fn inbound(
    model: &mut Model,
    env: &Env<Limits>,
    run: Token,
    attempt: Token,
    event: Box<[u8]>,
    out: &mut Queue<Request>,
) {
    let Some(id) = fenced(&model.names, &model.hosted, run, attempt) else {
        return;
    };
    if len(&event) > env.limits.event_bytes {
        out.push(Request::Bounced { run, attempt, bounce: Bounce::TooLarge });
        return;
    }
    let entry = model.hosted.get_mut(id).expect("a named run is hosted");
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Preparing { reply_to, charter, snapshot, mut held } => {
            hold(&mut held, event, run, attempt, out);
            State::Preparing { reply_to, charter, snapshot, held }
        }
        State::Starting { reply_to, workspace, mut held } => {
            hold(&mut held, event, run, attempt, out);
            State::Starting { reply_to, workspace, held }
        }
        State::Active { reply_to, workspace, agent } | State::Waiting { reply_to, workspace, agent } => {
            out.push(Request::Deliver { agent, event });
            State::Active { reply_to, workspace, agent }
        }
        state @ (State::Cancelling { .. } | State::Unwanted { .. } | State::Stopping { .. } | State::Saving { .. }) => {
            out.push(Request::Bounced { run, attempt, bounce: Bounce::Ending });
            state
        }
        State::Closed => unreachable!("a closed run has left the names"),
    };
    conclude(model, id, out);
}

pub(crate) fn cancel(model: &mut Model, env: &Env<Limits>, run: Token, attempt: Token, out: &mut Queue<Request>) {
    let Some(id) = fenced(&model.names, &model.hosted, run, attempt) else {
        return;
    };
    stop(model, env, id, Reason::Engine, out);
}

pub(crate) fn relayed(
    model: &mut Model,
    run: Token,
    attempt: Token,
    call: Token,
    answer: Box<[u8]>,
    out: &mut Queue<Request>,
) {
    let Some(id) = fenced(&model.names, &model.hosted, run, attempt) else {
        return;
    };
    let Model { hosted, names: _, calls, ready: _, facts: _ } = model;
    // The engine echoes the host's name for the call, which may be of a call
    // answered already, or not of this run.
    let call_id = Id::<Call>::from_token(call);
    let Some(entry) = calls.get_mut(call_id) else {
        return;
    };
    if entry.hosted != id {
        return;
    }
    match entry.state {
        call::State::Relayed { agent, call } => {
            out.push(Request::Reply { agent, call, reply: Reply::Relayed { answer } });
            entry.state = call::State::Closed;
            calls.retire(call_id);
            let entry = hosted.get_mut(id).expect("a named run is hosted");
            entry.calls.remove(&call_id);
        }
        // Not a relayed call, or answered already: dropped.
        call::State::Pushing { .. } | call::State::Orphaned | call::State::Closed => {}
    }
}

pub(crate) fn cancel_all(model: &mut Model, reason: Reason) {
    let Model { hosted: _, names, calls: _, ready, facts: _ } = model;
    for (_, id) in names.iter() {
        if !ready.contains_key(id) {
            let earlier = ready.insert(*id, reason).expect("room on the ready list for every run");
            assert!(earlier.is_none(), "checked the run is not ready above");
        }
    }
}

/// Cancels the first run on the ready list, if there is one.
pub(crate) fn resume(model: &mut Model, env: &Env<Limits>, out: &mut Queue<Request>) {
    let Some((id, reason)) = model.ready.first() else {
        return;
    };
    let (id, reason) = (*id, *reason);
    model.ready.remove(&id);
    stop(model, env, id, reason, out);
}

pub(crate) fn report(model: &Model, out: &mut Queue<Request>) {
    let mut runs = List::with_capacity(model.names.len());
    for (_, id) in &model.names {
        let entry = model.hosted.get(*id).expect("a named run is hosted");
        let hosting = Hosting { run: entry.run, attempt: entry.attempt, phase: phase(&entry.state) };
        runs.push(hosting).expect("room for every named run");
    }
    out.push(Request::Hosting { runs: runs.into_boxed() });
}

pub(crate) fn prepared(model: &mut Model, owner: Token, workspace: Token, out: &mut Queue<Request>) {
    let Model { hosted, names: _, calls: _, ready: _, facts } = model;
    let id = Id::<Hosted>::from_token(owner);
    let entry = hosted.get_mut(id).expect("a run lives until its prepare has settled");
    facts.push(Fact::Prepared { run: entry.run, attempt: entry.attempt });
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Preparing { reply_to, charter, snapshot, held } => {
            out.push(Request::Start { owner, workspace, charter, snapshot });
            State::Starting { reply_to, workspace, held }
        }
        State::Cancelling { reply_to, reason } => {
            let ending = Ending::Failed { failure: Failure::Cancelled(reason), detail: Box::new([]) };
            release(entry, facts, reply_to, workspace, ending, None, out)
        }
        State::Starting { .. }
        | State::Unwanted { .. }
        | State::Active { .. }
        | State::Waiting { .. }
        | State::Stopping { .. }
        | State::Saving { .. }
        | State::Closed => unreachable!("a workspace is prepared once, while its run prepares"),
    };
    conclude(model, id, out);
}

pub(crate) fn unprepared(
    model: &mut Model,
    env: &Env<Limits>,
    owner: Token,
    failure: Preparation,
    detail: Box<[u8]>,
    out: &mut Queue<Request>,
) {
    let Model { hosted, names: _, calls: _, ready: _, facts } = model;
    let id = Id::<Hosted>::from_token(owner);
    let entry = hosted.get_mut(id).expect("a run lives until its prepare has settled");
    let detail = tail(detail, &env.limits);
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        // Nothing of the workspace is held: nothing to release.
        State::Preparing { reply_to, .. } => {
            let ending = Ending::Failed { failure: Failure::Unprepared(failure), detail };
            answer(entry, facts, reply_to, ending, None, out)
        }
        State::Cancelling { reply_to, reason } => {
            let ending = Ending::Failed { failure: Failure::Cancelled(reason), detail };
            answer(entry, facts, reply_to, ending, None, out)
        }
        State::Starting { .. }
        | State::Unwanted { .. }
        | State::Active { .. }
        | State::Waiting { .. }
        | State::Stopping { .. }
        | State::Saving { .. }
        | State::Closed => unreachable!("a workspace is prepared once, while its run prepares"),
    };
    conclude(model, id, out);
}

pub(crate) fn started(model: &mut Model, env: &Env<Limits>, owner: Token, agent: Token, out: &mut Queue<Request>) {
    let Model { hosted, names: _, calls: _, ready: _, facts } = model;
    let id = Id::<Hosted>::from_token(owner);
    let entry = hosted.get_mut(id).expect("a run lives until its agent has gone");
    facts.push(Fact::Started { run: entry.run, attempt: entry.attempt });
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Starting { reply_to, workspace, mut held } => {
            // Bounded: the hold has room for the limit, and no more.
            for _ in 0..env.limits.held {
                let Some(event) = held.pop() else {
                    break;
                };
                out.push(Request::Deliver { agent, event });
            }
            State::Active { reply_to, workspace, agent }
        }
        State::Unwanted { reply_to, workspace, reason } => {
            out.push(Request::Stop { agent });
            let ending = Ending::Failed { failure: Failure::Cancelled(reason), detail: Box::new([]) };
            State::Stopping { reply_to, workspace, agent, ending, gone: false }
        }
        State::Preparing { .. }
        | State::Cancelling { .. }
        | State::Active { .. }
        | State::Waiting { .. }
        | State::Stopping { .. }
        | State::Saving { .. }
        | State::Closed => unreachable!("an agent starts once, after its start"),
    };
    conclude(model, id, out);
}

pub(crate) fn called(
    model: &mut Model,
    env: &Env<Limits>,
    owner: Token,
    call: Token,
    ask: Ask,
    out: &mut Queue<Request>,
) {
    let Model { hosted, names: _, calls, ready: _, facts: _ } = model;
    let id = Id::<Hosted>::from_token(owner);
    let entry = hosted.get_mut(id).expect("a run lives until its agent has gone");
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        // A run that calls is at work, whatever it said before.
        State::Active { reply_to, workspace, agent } | State::Waiting { reply_to, workspace, agent } => {
            serve(entry.run, entry.attempt, &mut entry.calls, calls, id, workspace, agent, call, ask, &env.limits, out);
            State::Active { reply_to, workspace, agent }
        }
        // Once a run is cancelled or ending, its calls are not served.
        State::Stopping { reply_to, workspace, agent, ending, gone } => {
            out.push(Request::Reply { agent, call, reply: Reply::Unavailable });
            State::Stopping { reply_to, workspace, agent, ending, gone }
        }
        State::Preparing { .. }
        | State::Cancelling { .. }
        | State::Starting { .. }
        | State::Unwanted { .. }
        | State::Saving { .. }
        | State::Closed => unreachable!("only an agent that has started and not gone calls"),
    };
    conclude(model, id, out);
}

pub(crate) fn yielded(model: &mut Model, owner: Token, out: &mut Queue<Request>) {
    let id = Id::<Hosted>::from_token(owner);
    let entry = model.hosted.get_mut(id).expect("a run lives until its agent has gone");
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Active { reply_to, workspace, agent } | State::Waiting { reply_to, workspace, agent } => {
            State::Waiting { reply_to, workspace, agent }
        }
        state @ State::Stopping { .. } => state,
        State::Preparing { .. }
        | State::Cancelling { .. }
        | State::Starting { .. }
        | State::Unwanted { .. }
        | State::Saving { .. }
        | State::Closed => unreachable!("only an agent that has started and not gone yields"),
    };
    conclude(model, id, out);
}

pub(crate) fn finished(model: &mut Model, env: &Env<Limits>, owner: Token, finish: Finish, out: &mut Queue<Request>) {
    let Model { hosted, names: _, calls, ready: _, facts: _ } = model;
    let id = Id::<Hosted>::from_token(owner);
    let entry = hosted.get_mut(id).expect("a run lives until its agent has gone");
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Active { reply_to, workspace, agent } | State::Waiting { reply_to, workspace, agent } => {
            let ending = ending(finish, &env.limits);
            leave(&mut entry.calls, calls, &env.limits, out);
            out.push(Request::Stop { agent });
            State::Stopping { reply_to, workspace, agent, ending, gone: false }
        }
        // How the run ends is decided already.
        state @ State::Stopping { .. } => state,
        State::Preparing { .. }
        | State::Cancelling { .. }
        | State::Starting { .. }
        | State::Unwanted { .. }
        | State::Saving { .. }
        | State::Closed => unreachable!("only an agent that has started and not gone finishes"),
    };
    conclude(model, id, out);
}

pub(crate) fn faulted(
    model: &mut Model,
    env: &Env<Limits>,
    owner: Token,
    fault: AgentFailure,
    out: &mut Queue<Request>,
) {
    let Model { hosted, names: _, calls, ready: _, facts: _ } = model;
    let id = Id::<Hosted>::from_token(owner);
    let entry = hosted.get_mut(id).expect("a run lives until its agent has gone");
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Active { reply_to, workspace, agent } | State::Waiting { reply_to, workspace, agent } => {
            let ending = Ending::Failed { failure: Failure::Agent(fault), detail: Box::new([]) };
            leave(&mut entry.calls, calls, &env.limits, out);
            out.push(Request::Stop { agent });
            State::Stopping { reply_to, workspace, agent, ending, gone: false }
        }
        // How the run ends is decided already.
        state @ State::Stopping { .. } => state,
        State::Preparing { .. }
        | State::Cancelling { .. }
        | State::Starting { .. }
        | State::Unwanted { .. }
        | State::Saving { .. }
        | State::Closed => unreachable!("only an agent that has started and not gone is faulted"),
    };
    conclude(model, id, out);
}

pub(crate) fn gone(model: &mut Model, env: &Env<Limits>, owner: Token, detail: Box<[u8]>, out: &mut Queue<Request>) {
    let Model { hosted, names: _, calls, ready: _, facts } = model;
    let id = Id::<Hosted>::from_token(owner);
    let entry = hosted.get_mut(id).expect("a run lives until its agent has gone");
    let detail = tail(detail, &env.limits);
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        // Nothing ran: nothing to save.
        State::Starting { reply_to, workspace, held: _ } => {
            let ending = Ending::Failed { failure: Failure::Agent(AgentFailure::Unstarted), detail };
            release(entry, facts, reply_to, workspace, ending, None, out)
        }
        State::Unwanted { reply_to, workspace, reason } => {
            let ending = Ending::Failed { failure: Failure::Cancelled(reason), detail };
            release(entry, facts, reply_to, workspace, ending, None, out)
        }
        // It exited without saying how its run finishes.
        State::Active { reply_to, workspace, agent } | State::Waiting { reply_to, workspace, agent } => {
            let ending = Ending::Failed { failure: Failure::Agent(AgentFailure::Exited), detail };
            leave(&mut entry.calls, calls, &env.limits, out);
            State::Stopping { reply_to, workspace, agent, ending, gone: true }
        }
        State::Stopping { reply_to, workspace, agent, ending, gone } => {
            assert!(!gone, "an agent goes once");
            let ending = explained(ending, detail);
            State::Stopping { reply_to, workspace, agent, ending, gone: true }
        }
        State::Preparing { .. } | State::Cancelling { .. } | State::Saving { .. } | State::Closed => {
            unreachable!("only an agent that was started goes")
        }
    };
    conclude(model, id, out);
}

pub(crate) fn pushed(model: &mut Model, owner: Token, push: Box<[Landing]>, out: &mut Queue<Request>) {
    let Model { hosted, names: _, calls, ready: _, facts: _ } = model;
    let call_id = Id::<Call>::from_token(owner);
    let call = calls.get_mut(call_id).expect("a push's call lives until the push has settled");
    let id = call.hosted;
    let state = mem::replace(&mut call.state, call::State::Closed);
    match state {
        call::State::Pushing { agent, call } => {
            out.push(Request::Reply { agent, call, reply: Reply::Pushed(told(&push)) });
        }
        // Its run left live, and its call was answered then.
        call::State::Orphaned => {}
        call::State::Relayed { .. } | call::State::Closed => unreachable!("a push ends once, and only a push's call"),
    }
    calls.retire(call_id);
    let entry = hosted.get_mut(id).expect("a run lives until its pushes have settled");
    entry.calls.remove(&call_id);
    // Bounded: no more than the repositories the workspace lists.
    for (landing, index) in push.iter().zip(0..entry.repositories) {
        match landing {
            Landing::Landed => {
                entry.landed.insert(index).expect("room for every repository");
            }
            Landing::Moved | Landing::Failed | Landing::Unchanged => {}
        }
    }
    conclude(model, id, out);
}

pub(crate) fn saved(model: &mut Model, owner: Token, save: Box<[Landing]>, out: &mut Queue<Request>) {
    let Model { hosted, names: _, calls: _, ready: _, facts } = model;
    let id = Id::<Hosted>::from_token(owner);
    let entry = hosted.get_mut(id).expect("a run lives until its save has settled");
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Saving { reply_to, workspace, ending } => {
            let save = cut(save, entry.repositories);
            release(entry, facts, reply_to, workspace, ending, Some(save), out)
        }
        State::Preparing { .. }
        | State::Cancelling { .. }
        | State::Starting { .. }
        | State::Unwanted { .. }
        | State::Active { .. }
        | State::Waiting { .. }
        | State::Stopping { .. }
        | State::Closed => unreachable!("a save ends once, while its run saves"),
    };
    conclude(model, id, out);
}

/// The hosted run the engine names `run`, if it hosts that attempt at it.
fn fenced(names: &Map<Token, Id<Hosted>>, hosted: &Slab<Hosted>, run: Token, attempt: Token) -> Option<Id<Hosted>> {
    let id = *names.get(&run)?;
    let entry = hosted.get(id).expect("a named run is hosted");
    (entry.attempt == attempt).then_some(id)
}

/// Cancels the run `id` for `reason`: the cancel cells.
fn stop(model: &mut Model, env: &Env<Limits>, id: Id<Hosted>, reason: Reason, out: &mut Queue<Request>) {
    let Model { hosted, names: _, calls, ready: _, facts: _ } = model;
    let entry = hosted.get_mut(id).expect("a run on the names or the ready list is hosted");
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        // A prepare in flight is waited for.
        State::Preparing { reply_to, .. } => State::Cancelling { reply_to, reason },
        State::Starting { reply_to, workspace, held: _ } => State::Unwanted { reply_to, workspace, reason },
        State::Active { reply_to, workspace, agent } | State::Waiting { reply_to, workspace, agent } => {
            let ending = Ending::Failed { failure: Failure::Cancelled(reason), detail: Box::new([]) };
            leave(&mut entry.calls, calls, &env.limits, out);
            out.push(Request::Stop { agent });
            State::Stopping { reply_to, workspace, agent, ending, gone: false }
        }
        // How the run ends is decided already.
        state @ (State::Cancelling { .. } | State::Unwanted { .. } | State::Stopping { .. } | State::Saving { .. }) => {
            state
        }
        State::Closed => unreachable!("a closed run has left the names and the ready list"),
    };
    conclude(model, id, out);
}

/// What a run's state implies, applied after every transition: a stopping run
/// that has settled saves, or releases its workspace and answers; and a
/// Closed run is retired, leaving the names and the ready list.
fn conclude(model: &mut Model, id: Id<Hosted>, out: &mut Queue<Request>) {
    let Model { hosted, names, calls: _, ready, facts } = model;
    let entry = hosted.get_mut(id).expect("a run lives until it is retired");
    let settled = match &entry.state {
        State::Stopping { gone, .. } => *gone && entry.calls.is_empty(),
        State::Preparing { .. }
        | State::Cancelling { .. }
        | State::Starting { .. }
        | State::Unwanted { .. }
        | State::Active { .. }
        | State::Waiting { .. }
        | State::Saving { .. }
        | State::Closed => false,
    };
    if settled {
        let state = mem::replace(&mut entry.state, State::Closed);
        entry.state = match state {
            State::Stopping { reply_to, workspace, agent: _, ending, gone: _ } => {
                settle(entry, facts, id, reply_to, workspace, ending, out)
            }
            State::Preparing { .. }
            | State::Cancelling { .. }
            | State::Starting { .. }
            | State::Unwanted { .. }
            | State::Active { .. }
            | State::Waiting { .. }
            | State::Saving { .. }
            | State::Closed => unreachable!("only a stopping run settles"),
        };
    }
    let closed = match &entry.state {
        State::Closed => true,
        State::Preparing { .. }
        | State::Cancelling { .. }
        | State::Starting { .. }
        | State::Unwanted { .. }
        | State::Active { .. }
        | State::Waiting { .. }
        | State::Stopping { .. }
        | State::Saving { .. } => false,
    };
    if closed {
        let run = entry.run;
        names.remove(&run);
        ready.remove(&id);
        hosted.retire(id);
    }
}

// Cell handlers and what they share.

/// Stopping, settled: nothing of the run is running and nothing it asked of
/// the workspace is in flight. Its work is saved if the assignment asks and
/// none of it landed; otherwise the run releases its workspace and answers.
fn settle(
    entry: &mut Hosted,
    facts: &mut Facts,
    id: Id<Hosted>,
    reply_to: ReplyTo,
    workspace: Token,
    ending: Ending,
    out: &mut Queue<Request>,
) -> State {
    let save = if entry.landed.is_empty() { entry.save.take() } else { None };
    match save {
        Some(branch) => {
            out.push(Request::Save { owner: id.token(), workspace, branch });
            State::Saving { reply_to, workspace, ending }
        }
        None => release(entry, facts, reply_to, workspace, ending, None, out),
    }
}

/// Releases the run's workspace and answers.
fn release(
    entry: &Hosted,
    facts: &mut Facts,
    reply_to: ReplyTo,
    workspace: Token,
    ending: Ending,
    saved: Option<Box<[Landing]>>,
    out: &mut Queue<Request>,
) -> State {
    out.push(Request::Release { workspace });
    answer(entry, facts, reply_to, ending, saved, out)
}

/// Answers the engine: the run's one answer.
fn answer(
    entry: &Hosted,
    facts: &mut Facts,
    reply_to: ReplyTo,
    ending: Ending,
    saved: Option<Box<[Landing]>>,
    out: &mut Queue<Request>,
) -> State {
    let (run, attempt) = (entry.run, entry.attempt);
    let mut landed = List::with_capacity(entry.landed.len());
    for index in &entry.landed {
        landed.push(*index).expect("room for every repository landed in");
    }
    let work = Work { landed: landed.into_boxed(), saved };
    let answer = match ending {
        Ending::Ended { outcome } => {
            facts.push(Fact::Ended { run, attempt });
            Answer::Ended { outcome, work }
        }
        Ending::Parked { snapshot } => {
            facts.push(Fact::Parked { run, attempt });
            Answer::Parked { snapshot, work }
        }
        Ending::Failed { failure, detail } => {
            facts.push(Fact::Failed { run, attempt, failure });
            Answer::Failed { failure, detail, work }
        }
    };
    out.push(Request::Answer { to: reply_to, run, attempt, answer });
    State::Closed
}

fn refuse(reply_to: ReplyTo, assignment: &Assignment, refusal: Refusal, out: &mut Queue<Request>) {
    let (run, attempt) = (assignment.run, assignment.attempt);
    out.push(Request::Answer { to: reply_to, run, attempt, answer: Answer::Refused(refusal) });
}

/// Holds `event` until the run is live, or bounces it if the hold is full.
fn hold(held: &mut Queue<Box<[u8]>>, event: Box<[u8]>, run: Token, attempt: Token, out: &mut Queue<Request>) {
    if held.try_push(event).is_err() {
        out.push(Request::Bounced { run, attempt, bounce: Bounce::Full });
    }
}

/// Serves the host call `call` of the live run `id`, through its workspace
/// or the engine, or answers it as busy.
#[expect(clippy::too_many_arguments, reason = "a cell handler takes the fields it touches")]
fn serve(
    run: Token,
    attempt: Token,
    run_calls: &mut Set<Id<Call>>,
    calls: &mut Slab<Call>,
    id: Id<Hosted>,
    workspace: Token,
    agent: Token,
    call: Token,
    ask: Ask,
    limits: &Limits,
    out: &mut Queue<Request>,
) {
    // A push is a write: one at a time.
    let mut pushing = false;
    for call_id in run_calls.iter() {
        let entry = calls.get(*call_id).expect("a run's calls live until they close");
        match entry.state {
            call::State::Pushing { .. } | call::State::Orphaned => pushing = true,
            call::State::Relayed { .. } | call::State::Closed => {}
        }
    }
    let is_push = match &ask {
        Ask::Push { .. } => true,
        Ask::Relay { .. } => false,
    };
    if run_calls.len() >= limits.run_calls || (is_push && pushing) {
        out.push(Request::Reply { agent, call, reply: Reply::Busy });
        return;
    }
    let state = if is_push { call::State::Pushing { agent, call } } else { call::State::Relayed { agent, call } };
    // Calls answered in this iteration keep their slots until the reclaim
    // point: a call may find none free even within its run's limit.
    let Ok(call_id) = calls.insert(Call { hosted: id, state }) else {
        out.push(Request::Reply { agent, call, reply: Reply::Busy });
        return;
    };
    let added = run_calls.insert(call_id).expect("checked the run's calls for room above");
    assert!(added, "a call is new to its run");
    match ask {
        Ask::Push { message } => out.push(Request::Push { owner: call_id.token(), workspace, message }),
        Ask::Relay { body } => out.push(Request::Relay { run, attempt, call: call_id.token(), body }),
    }
}

/// The run leaves live: each of its calls in flight is answered as
/// unavailable, a relayed one closing and a push becoming orphaned.
fn leave(run_calls: &mut Set<Id<Call>>, calls: &mut Slab<Call>, limits: &Limits, out: &mut Queue<Request>) {
    let mut orphaned = Set::with_capacity(limits.run_calls);
    for call_id in run_calls.iter() {
        let entry = calls.get_mut(*call_id).expect("a run's calls live until they close");
        let state = mem::replace(&mut entry.state, call::State::Closed);
        entry.state = match state {
            call::State::Pushing { agent, call } => {
                out.push(Request::Reply { agent, call, reply: Reply::Unavailable });
                call::State::Orphaned
            }
            call::State::Relayed { agent, call } => {
                out.push(Request::Reply { agent, call, reply: Reply::Unavailable });
                call::State::Closed
            }
            call::State::Orphaned | call::State::Closed => unreachable!("a run leaves live once"),
        };
        match entry.state {
            call::State::Orphaned => {
                orphaned.insert(*call_id).expect("room for the run's calls");
            }
            call::State::Closed => calls.retire(*call_id),
            call::State::Pushing { .. } | call::State::Relayed { .. } => unreachable!("set above"),
        }
    }
    *run_calls = orphaned;
}

/// How the run ends, as it says it finishes. Saying more than the limits allow
/// breaks the rules.
fn ending(finish: Finish, limits: &Limits) -> Ending {
    let rules = Ending::Failed { failure: Failure::Agent(AgentFailure::Rules), detail: Box::new([]) };
    match finish {
        Finish::Ended { outcome } if len(&outcome) > limits.outcome_bytes => rules,
        Finish::Ended { outcome } => Ending::Ended { outcome },
        Finish::Parked { snapshot: Some(snapshot) } if len(&snapshot) > limits.snapshot_bytes => rules,
        Finish::Parked { snapshot } => Ending::Parked { snapshot },
        Finish::Failed { failure } => Ending::Failed { failure: Failure::Run(failure), detail: Box::new([]) },
    }
}

/// A failure's ending gains the detail its agent left as it went.
fn explained(ending: Ending, detail: Box<[u8]>) -> Ending {
    match ending {
        Ending::Failed { failure, detail: _ } => Ending::Failed { failure, detail },
        ending @ (Ending::Ended { .. } | Ending::Parked { .. }) => ending,
    }
}

/// What the run is told of a push: done only if every repository with a
/// change landed it; moved if any branch moved.
fn told(push: &[Landing]) -> Push {
    let (mut landed, mut moved, mut failed) = (false, false, false);
    for landing in push {
        match landing {
            Landing::Landed => landed = true,
            Landing::Moved => moved = true,
            Landing::Failed => failed = true,
            Landing::Unchanged => {}
        }
    }
    if moved {
        Push::Moved
    } else if failed {
        Push::Failed
    } else if landed {
        Push::Done
    } else {
        Push::Nothing
    }
}

/// The last bytes of `detail`, as many as the limits keep.
fn tail(detail: Box<[u8]>, limits: &Limits) -> Box<[u8]> {
    let keep = usize::try_from(limits.detail_bytes).expect("a u32 fits in a usize");
    match detail.len().checked_sub(keep) {
        Some(cut) if cut > 0 => copy_of(detail.get(cut..).expect("cut within the detail")),
        Some(_) | None => detail,
    }
}

/// A save's outcome for the repositories the workspace lists, and no more.
fn cut(save: Box<[Landing]>, repositories: u32) -> Box<[Landing]> {
    let count = usize::try_from(repositories).expect("a u32 fits in a usize");
    if save.len() <= count {
        return save;
    }
    let mut kept = List::with_capacity(repositories);
    for landing in save.iter().take(count) {
        kept.push(*landing).expect("room for every repository");
    }
    kept.into_boxed()
}

fn phase(state: &State) -> Phase {
    match state {
        State::Preparing { .. } => Phase::Preparing,
        State::Starting { .. } => Phase::Starting,
        State::Active { .. } => Phase::Active,
        State::Waiting { .. } => Phase::Waiting,
        State::Cancelling { .. } | State::Unwanted { .. } | State::Stopping { .. } | State::Saving { .. } => {
            Phase::Ending
        }
        State::Closed => unreachable!("a closed run has left the names"),
    }
}
