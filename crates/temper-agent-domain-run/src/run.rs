//! Runs: one agent instance, from its admission to its one answer.
//!
//! A `Start` admits a run, or refuses it at the entrance. An admitted run
//! first looks in its checkout for what it tells the LLM and what it checks
//! (the `prepare` module), then opens its main conversation and drives it:
//! when the LLM stops without finishing, the run nudges it, within its nudges
//! and its budget; when it calls `finish`, the run judges the outcome, and
//! lands a change (the `land` module). It works until how it ends is decided;
//! then it closes main, waits for main to end, and answers. The first ending
//! decided wins, but for a change that lands: once it is pushed, it is on the
//! forge, and the run finishes with it whatever it was winding down for. The
//! run answers only once nothing it started is still in flight.
//!
//! A run that spends past its budget does not cut main off mid-turn: main
//! keeps the turn in flight (Over), and is closed at its next turn or yield,
//! or when a finish it called in that turn is refused, unless that finish is
//! accepted first.
//!
//! A run's transition table:
//!
//! ```text
//! state      event or alarm                next       emits
//! -          start, beyond the limits      -          answer: invalid
//!            start, no room                -          answer: busy
//!            start                         Preparing  admitted, the first look
//!            start, nothing to look for    Working    admitted, open main
//! Preparing  read, probed                  Preparing  the next look
//!            read, probed, the last        Working    open main
//!            deadline                      Stopping   (out of time)
//!            cancel                        Stopping   (cancelled)
//! Stopping   read, probed                  Closed     answer: the ending decided
//!            cancel                        Stopping
//! Working    main yielded                  Working    say: a nudge
//!            main yielded, no nudge left   Winding    close main: unfinished
//!            main yielded, no turn left    Winding    close main: out of budget
//!            used, past the budget         Over
//!            finish, refused               Working    return: rejected
//!            finish, a verdict             Winding    return: accepted, close main
//!            finish, a change              Working    (landing)
//!            landed: pushed                Winding    close main: accepted
//!            landed: refused               Working
//!            landed: moved                 Winding    close main: stale
//!            deadline                      Winding    close main: out of time
//!            cancel                        Winding    close main: cancelled
//!            main ended                    Closed     answer: how main ended
//! Over       used, main yielded            Winding    close main: out of budget
//!            finish, refused               Winding    return: rejected, close main
//!            finish, a verdict             Winding    return: accepted, close main
//!            finish, a change              Over       (landing)
//!            landed: pushed                Winding    close main: accepted
//!            landed: refused               Winding    close main: out of budget
//!            landed: moved                 Winding    close main: stale
//!            deadline                      Winding    close main: out of budget
//!            cancel                        Winding    close main: cancelled
//!            main ended                    Closed     answer: how main ended
//! Winding    landed: pushed                Winding    (accepted)
//!            used, cancel, landed          Winding
//!            finish                        Winding    return: cancelled
//!            main ended                    Closed     answer: the ending decided
//! Closed     cancel                        Closed
//! ```
//!
//! and a conversation's, as the run keeps it:
//!
//! ```text
//! state     event or call                next
//! Pending   opened by its run            Opening   open it
//!           the run stops                Closed
//! Opening   started                      Running
//!           closed by its run            Unwanted
//!           ended (refused)              Closed
//! Unwanted  started                      Closing   close it
//!           ended (refused)              Closed
//! Running   yielded, used, delegated     Running
//!           closed by its run            Closing   close it
//!           ended                        Closed
//! Closing   yielded, used, delegated     Closing
//!           ended                        Closed
//! ```
//!
//! Every other cell is unreachable by the contracts: a conversation's
//! (`Started` first unless refused, then events, then one `Ended` once its
//! calls have returned and what was in flight has settled), io's and the
//! worker's (one terminal per request, one look at a time while a run
//! prepares). A yield is decided in the step it arrives, so a conversation
//! never rests yielded; a finish is a write, which a conversation runs alone,
//! so it has at most one landing at a time. The deadline alarm runs while a
//! run prepares, works or is over its budget; it follows from the state, in
//! one place ([`follow`]), which also retires a run once it is Closed. A
//! call's alarm runs from the call's start until it returns or is withdrawn.

use alloc::boxed::Box;
use core::mem;

use skein_lib::bytes::copy_of;
use skein_lib::{Deadlines, Duration, Env, Id, Queue, ReplyTo, Slab, Time, Token};

use crate::agent::{self, Child, Means};
use crate::boundary::{
    Answer, Ask, AskRefusal, End, Failure, Fault, Invalid, Opening, Policy, Push, Ran, Read, Refusal, Request,
    Returned, Stop,
};
use crate::budget::{Exhausted, Spend};
use crate::call::{Call, Calls, Withdrawal, Work};
use crate::charter::{self, Charter, Families, count};
use crate::domain::Domain;
use crate::facts::{Asked, Fact};
use crate::land::{self, Settled};
use crate::limits::Limits;
use crate::outcome::{self, Declared};
use crate::prepare::{self, Found, Step};
use crate::prompt;

#[derive(Debug)]
pub(crate) struct Run {
    pub(crate) charter: Charter,
    /// What it found in its checkout as it prepared.
    pub(crate) found: Found,
    /// The worker's name for it.
    pub(crate) worker: Token,
    /// What its conversations have spent.
    spent: Spend,
    /// Nudges given.
    nudges: u32,
    /// Outcomes `finish` refused: rejected, or not landed.
    rejected: u32,
    /// Its conversations, main included, opened and not ended.
    conversations: u32,
    /// When its budget's time runs out.
    deadline: Time,
    state: State,
}

#[derive(Debug)]
enum State {
    /// Looking in its checkout: the look `step` is in flight. Its main
    /// conversation has its slot, and is not opened yet.
    Preparing { reply_to: ReplyTo, main: Id<Conversation>, step: Step },
    /// It fails with `failure` once the look in flight has ended.
    Stopping { reply_to: ReplyTo, main: Id<Conversation>, failure: Failure },
    /// Its main conversation is at work.
    Working { reply_to: ReplyTo, main: Id<Conversation> },
    /// It has spent past its budget's `exhausted` part, and main keeps the
    /// turn in flight.
    Over { reply_to: ReplyTo, main: Id<Conversation>, exhausted: Exhausted },
    /// It ends with `ending` once its main conversation has ended.
    Winding { reply_to: ReplyTo, ending: Ending },
    /// Terminal: holds nothing.
    Closed,
}

/// How a winding run ends.
#[derive(Debug)]
enum Ending {
    Accepted(Declared),
    Failed(Failure),
}

/// A conversation a run opened: main, or a sub-agent.
#[derive(Debug)]
pub(crate) struct Conversation {
    run: Id<Run>,
    /// The sub-agent call it serves, if it is a sub-agent.
    asker: Option<Id<Call>>,
    /// Its families of tools, and how deep it is: main is at zero, a sub-agent
    /// one deeper than its asker.
    families: Families,
    depth: u32,
    /// What it has spent, by its `Used` so far.
    spent: Spend,
    /// Its calls to the run in flight.
    calls: u32,
    phase: Phase,
}

#[derive(Debug)]
enum Phase {
    /// Not opened yet: its run is preparing.
    Pending,
    /// Opened, and not started yet.
    Opening,
    /// Opened, not started yet, and no longer wanted: closed once it starts.
    Unwanted,
    /// Started, and addressed as `peer`.
    Running { peer: Token },
    /// Closed by its run, and not ended yet.
    Closing,
    /// Terminal: holds nothing.
    Closed,
}

/// The timers of runs and their calls, named by what they are for.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum Alarm {
    /// The budget's time of the run `run` runs out.
    Deadline { run: Id<Run> },
    /// The deadline of the call `call` passes.
    Call { call: Id<Call> },
}

// Entry points, one per event or alarm: look the conversation or run up, take
// its state out, run the cell's handler, follow the run's new state.

pub(crate) fn start(
    domain: &mut Domain,
    env: &Env<Limits>,
    reply_to: ReplyTo,
    worker: Token,
    charter: Charter,
    out: &mut Queue<Request>,
) {
    let Domain { runs, conversations, calls: _, alarms, facts } = domain;
    // A charter that can never fit is invalid, room or not: busy invites a
    // retry.
    if let Err(invalid) = charter::check(&charter, &env.limits) {
        out.push(Request::Answer { to: reply_to, answer: Answer::Refused(Refusal::Invalid(invalid)) });
        return;
    }
    if runs.is_full() || conversations.is_full() {
        out.push(Request::Answer { to: reply_to, answer: Answer::Refused(Refusal::Busy) });
        return;
    }
    let deadline = env.now.saturating_add(charter.budget.time);
    let found = Found::with_capacity(count(charter.checkout.repositories.len()));
    // A run is stored before its main conversation, which names it, and starts
    // once main has a name too: main's slot is the run's from its admission.
    let run = Run {
        charter,
        found,
        worker,
        spent: Spend::ZERO,
        nudges: 0,
        rejected: 0,
        conversations: 1,
        deadline,
        state: State::Closed,
    };
    let id = runs.insert(run).expect("checked for room above");
    facts.about(id.token());
    let families = Families::of(&runs.get(id).expect("inserted above").charter.grants);
    let conversation =
        Conversation { run: id, asker: None, families, depth: 0, spent: Spend::ZERO, calls: 0, phase: Phase::Pending };
    let main = conversations.insert(conversation).expect("checked for room above");
    out.push(Request::Admitted { worker, run: id.token() });
    let run = runs.get_mut(id).expect("inserted above");
    run.state = match prepare::next(&run.charter, None) {
        Some(step) => look(run, id, reply_to, main, step, env, out),
        None => open(run, conversations, reply_to, main, env.now, out),
    };
    follow(runs, alarms, id);
}

pub(crate) fn read(domain: &mut Domain, env: &Env<Limits>, owner: Token, read: Read, out: &mut Queue<Request>) {
    let Domain { runs, conversations, calls: _, alarms, facts } = domain;
    let id = Id::<Run>::from_token(owner);
    facts.about(owner);
    let run = runs.get_mut(id).expect("a run lives until its look in flight has ended");
    let state = mem::replace(&mut run.state, State::Closed);
    run.state = match state {
        State::Preparing { reply_to, main, step } => {
            prepare::guide(&mut run.found, step, read, &env.limits);
            prepared(run, conversations, id, reply_to, main, step, env, out)
        }
        State::Stopping { reply_to, main, failure } => stop(run, conversations, reply_to, main, failure, out),
        State::Working { .. } | State::Over { .. } | State::Winding { .. } | State::Closed => {
            unreachable!("a run reads only while it prepares")
        }
    };
    follow(runs, alarms, id);
}

pub(crate) fn probed(domain: &mut Domain, env: &Env<Limits>, owner: Token, executable: bool, out: &mut Queue<Request>) {
    let Domain { runs, conversations, calls: _, alarms, facts } = domain;
    let id = Id::<Run>::from_token(owner);
    facts.about(owner);
    let run = runs.get_mut(id).expect("a run lives until its look in flight has ended");
    let state = mem::replace(&mut run.state, State::Closed);
    run.state = match state {
        State::Preparing { reply_to, main, step } => {
            prepare::checks(&mut run.found, step, executable);
            prepared(run, conversations, id, reply_to, main, step, env, out)
        }
        State::Stopping { reply_to, main, failure } => stop(run, conversations, reply_to, main, failure, out),
        State::Working { .. } | State::Over { .. } | State::Winding { .. } | State::Closed => {
            unreachable!("a run probes only while it prepares")
        }
    };
    follow(runs, alarms, id);
}

pub(crate) fn cancel(domain: &mut Domain, run: Token, out: &mut Queue<Request>) {
    let Domain { runs, conversations, calls: _, alarms, facts } = domain;
    let id = Id::<Run>::from_token(run);
    // A cancel travels down, so it may name a run that has answered and gone.
    let Some(run) = runs.get_mut(id) else {
        return;
    };
    facts.about(id.token());
    let cancelled = Ending::Failed(Failure::Cancelled);
    let state = mem::replace(&mut run.state, State::Closed);
    run.state = match state {
        State::Preparing { reply_to, main, step: _ } => State::Stopping { reply_to, main, failure: Failure::Cancelled },
        State::Working { reply_to, main } | State::Over { reply_to, main, exhausted: _ } => {
            wind_down(conversations, reply_to, main, cancelled, out)
        }
        // How the run ends is decided already.
        State::Stopping { reply_to, main, failure } => State::Stopping { reply_to, main, failure },
        State::Winding { reply_to, ending } => State::Winding { reply_to, ending },
        // It answered in this iteration, and is retired already.
        State::Closed => return,
    };
    follow(runs, alarms, id);
}

pub(crate) fn started(domain: &mut Domain, conversation: Token, peer: Token, out: &mut Queue<Request>) {
    let id = Id::<Conversation>::from_token(conversation);
    let conversation = domain.conversations.get_mut(id).expect("a conversation lives until it has ended");
    domain.facts.about(conversation.run.token());
    let phase = mem::replace(&mut conversation.phase, Phase::Closed);
    conversation.phase = match phase {
        Phase::Opening => Phase::Running { peer },
        Phase::Unwanted => closing(peer, out),
        Phase::Pending | Phase::Running { .. } | Phase::Closing | Phase::Closed => {
            unreachable!("a conversation starts once it is opened, before anything else")
        }
    };
}

pub(crate) fn yielded(
    domain: &mut Domain,
    env: &Env<Limits>,
    conversation: Token,
    stop: Stop,
    text: &[u8],
    out: &mut Queue<Request>,
) {
    let Domain { runs, conversations, calls, alarms, facts } = domain;
    let id = Id::<Conversation>::from_token(conversation);
    let conversation = conversations.get(id).expect("a conversation lives until it has ended");
    facts.about(conversation.run.token());
    let peer = match &conversation.phase {
        Phase::Running { peer } => *peer,
        // The yield crossed the run's close: there is nothing left to decide.
        Phase::Closing => return,
        Phase::Pending | Phase::Opening | Phase::Unwanted | Phase::Closed => {
            unreachable!("a conversation yields only between starting and ending")
        }
    };
    // A sub-agent that yields is done: its last message is its answer.
    if let Some(asker) = conversation.asker {
        let call = calls.get_mut(asker).expect("a sub-agent's call lives until it has ended");
        let close_child = match &mut call.work {
            Work::Child(child) => agent::yielded(child, text, stop, &env.limits),
            Work::Landing(_) => unreachable!("a sub-agent serves a sub-agent's call"),
        };
        if let Some(child) = close_child {
            close(conversations, child, out);
        }
        return;
    }
    let run_id = conversation.run;
    let run = runs.get_mut(run_id).expect("a run lives until its conversations have ended");
    let state = mem::replace(&mut run.state, State::Closed);
    run.state = match state {
        State::Working { reply_to, main } => {
            assert!(main == id, "a conversation with no asker is main");
            match nudge(run, stop, &env.limits) {
                Ok(()) => say(reply_to, main, peer, prompt::nudge(stop, run.nudges, env.limits.nudges), out),
                Err(failure) => wind_down(conversations, reply_to, main, Ending::Failed(failure), out),
            }
        }
        State::Over { reply_to, main, exhausted } => {
            wind_down(conversations, reply_to, main, Ending::Failed(Failure::Budget(exhausted)), out)
        }
        State::Preparing { .. } | State::Stopping { .. } | State::Winding { .. } | State::Closed => {
            unreachable!("main yields only while its run works: it is closed when the run winds down")
        }
    };
    follow(runs, alarms, run_id);
}

pub(crate) fn used(domain: &mut Domain, conversation: Token, spend: Spend, out: &mut Queue<Request>) {
    let Domain { runs, conversations, calls: _, alarms, facts } = domain;
    let id = Id::<Conversation>::from_token(conversation);
    let conversation = conversations.get_mut(id).expect("a conversation lives until it has ended");
    match &conversation.phase {
        Phase::Running { .. } | Phase::Closing => {}
        Phase::Pending | Phase::Opening | Phase::Unwanted | Phase::Closed => {
            unreachable!("a conversation spends only between starting and ending")
        }
    }
    conversation.spent = conversation.spent.saturating_add(spend);
    let (run_id, child) = (conversation.run, conversation.asker.is_some());
    facts.about(run_id.token());
    let run = runs.get_mut(run_id).expect("a run lives until its conversations have ended");
    run.spent = run.spent.saturating_add(spend);
    let state = mem::replace(&mut run.state, State::Closed);
    run.state = match state {
        // Past the budget, main keeps the turn now in flight: it may finish.
        // A sub-agent may not, and main has no turn in flight while it waits
        // on one: the run winds down at once.
        State::Working { reply_to, main } => match run.charter.budget.overspent(run.spent) {
            Some(exhausted) if child => {
                wind_down(conversations, reply_to, main, Ending::Failed(Failure::Budget(exhausted)), out)
            }
            Some(exhausted) => State::Over { reply_to, main, exhausted },
            None => State::Working { reply_to, main },
        },
        State::Over { reply_to, main, exhausted } => {
            wind_down(conversations, reply_to, main, Ending::Failed(Failure::Budget(exhausted)), out)
        }
        // Spent all the same, and counted in the answer.
        State::Winding { reply_to, ending } => State::Winding { reply_to, ending },
        State::Preparing { .. } | State::Stopping { .. } | State::Closed => {
            unreachable!("a run's conversation spends only while the run works or winds down")
        }
    };
    follow(runs, alarms, run_id);
}

pub(crate) fn delegated(
    domain: &mut Domain,
    env: &Env<Limits>,
    conversation: Token,
    call: Token,
    ask: Ask,
    deadline: Time,
    out: &mut Queue<Request>,
) {
    let Domain { runs, conversations, calls, alarms, facts } = domain;
    let id = Id::<Conversation>::from_token(conversation);
    let conversation = conversations.get(id).expect("a conversation lives until it has ended");
    let closing = match &conversation.phase {
        Phase::Running { .. } => false,
        Phase::Closing => true,
        Phase::Pending | Phase::Opening | Phase::Unwanted | Phase::Closed => {
            unreachable!("a conversation calls only between starting and ending")
        }
    };
    let run_id = conversation.run;
    facts.about(run_id.token());
    let asked = match &ask {
        Ask::Finish { .. } => Asked::Finish,
        Ask::SubAgent { .. } => Asked::SubAgent,
    };
    facts.push(Fact::Called { run: run_id.token(), conversation: id.token(), call, ask: asked });
    // The call crossed its conversation's close: it would be withdrawn at once.
    if closing {
        out.push(Request::Return { call, result: Returned::Cancelled });
        return;
    }
    let run = runs.get_mut(run_id).expect("a run lives until its conversations have ended");
    let state = mem::replace(&mut run.state, State::Closed);
    run.state = match state {
        // The call crossed the run's close.
        State::Winding { reply_to, ending } => {
            out.push(Request::Return { call, result: Returned::Cancelled });
            State::Winding { reply_to, ending }
        }
        State::Working { reply_to, main } => match ask {
            Ask::Finish { outcome } => {
                assert!(main == id, "only main is offered finish");
                let made = Asking { call, deadline };
                finish(run, run_id, conversations, calls, alarms, reply_to, main, None, made, outcome, env, out)
            }
            Ask::SubAgent { brief, families, llm, share } => {
                let wanted = Wanted { brief, families, llm, share };
                let made = Asking { call, deadline };
                sub_agent(run, run_id, conversations, calls, alarms, id, made, wanted, env, out);
                State::Working { reply_to, main }
            }
        },
        State::Over { reply_to, main, exhausted } => match ask {
            Ask::Finish { outcome } => {
                assert!(main == id, "only main is offered finish");
                let made = Asking { call, deadline };
                let over = Some(exhausted);
                finish(run, run_id, conversations, calls, alarms, reply_to, main, over, made, outcome, env, out)
            }
            // Past the budget, nothing new is opened.
            Ask::SubAgent { .. } => {
                out.push(Request::Return { call, result: Returned::Refused { refusal: AskRefusal::Over } });
                State::Over { reply_to, main, exhausted }
            }
        },
        State::Preparing { .. } | State::Stopping { .. } | State::Closed => {
            unreachable!("a run's conversation calls only while the run works or winds down")
        }
    };
    follow(runs, alarms, run_id);
}

pub(crate) fn withdraw(domain: &mut Domain, conversation: Token, call: Token, out: &mut Queue<Request>) {
    let conversation = Id::<Conversation>::from_token(conversation);
    // A withdraw of a call that has returned is stale.
    let Some(id) = domain.calls.find(conversation, call) else {
        return;
    };
    // The call is settling now: its deadline no longer matters.
    domain.alarms.cancel(Alarm::Call { call: id });
    stop_call(domain, id, Withdrawal::Withdrawn, out);
}

/// The deadline of the call `id` passed before it returned: stop what it is
/// doing.
pub(crate) fn expired(domain: &mut Domain, id: Id<Call>, out: &mut Queue<Request>) {
    stop_call(domain, id, Withdrawal::Expired, out);
}

pub(crate) fn checked(domain: &mut Domain, env: &Env<Limits>, owner: Token, ran: Ran, out: &mut Queue<Request>) {
    let id = Id::<Call>::from_token(owner);
    let call = domain.calls.get_mut(id).expect("a call lives until it returns");
    let run = domain.runs.get(call.run).expect("a run lives until its calls have returned");
    domain.facts.about(call.run.token());
    domain.facts.push(Fact::CheckFinished { run: call.run.token(), exit: ran.exit });
    let settled = match &mut call.work {
        Work::Landing(landing) => land::checked(landing, id, call.owner, run, may_finish(&run.state), ran, env, out),
        Work::Child(_) => unreachable!("io and the worker answer only a landing's requests"),
    };
    settle(domain, id, settled, out);
}

pub(crate) fn aborted(domain: &mut Domain, owner: Token, out: &mut Queue<Request>) {
    let id = Id::<Call>::from_token(owner);
    let call = domain.calls.get_mut(id).expect("a call lives until it returns");
    domain.facts.about(call.run.token());
    let settled = match &mut call.work {
        Work::Landing(landing) => land::aborted(landing, call.owner, out),
        Work::Child(_) => unreachable!("io and the worker answer only a landing's requests"),
    };
    settle(domain, id, settled, out);
}

pub(crate) fn pushed(domain: &mut Domain, owner: Token, push: Push, out: &mut Queue<Request>) {
    let id = Id::<Call>::from_token(owner);
    let call = domain.calls.get_mut(id).expect("a call lives until it returns");
    domain.facts.about(call.run.token());
    domain.facts.push(Fact::Pushed { run: call.run.token(), push });
    let settled = match &mut call.work {
        Work::Landing(landing) => land::pushed(landing, call.owner, push, out),
        Work::Child(_) => unreachable!("io and the worker answer only a landing's requests"),
    };
    settle(domain, id, settled, out);
}

pub(crate) fn host_cancelled(domain: &mut Domain, owner: Token, out: &mut Queue<Request>) {
    let id = Id::<Call>::from_token(owner);
    let call = domain.calls.get_mut(id).expect("a call lives until it returns");
    domain.facts.about(call.run.token());
    let settled = match &mut call.work {
        Work::Landing(landing) => land::host_cancelled(landing, call.owner, out),
        Work::Child(_) => unreachable!("io and the worker answer only a landing's requests"),
    };
    settle(domain, id, settled, out);
}

pub(crate) fn ended(domain: &mut Domain, conversation: Token, end: End, spend: Spend, out: &mut Queue<Request>) {
    let Domain { runs, conversations, calls, alarms, facts } = domain;
    let id = Id::<Conversation>::from_token(conversation);
    let conversation = conversations.get_mut(id).expect("a conversation lives until it has ended");
    assert!(conversation.calls == 0, "a conversation ends once its calls have returned");
    let phase = mem::replace(&mut conversation.phase, Phase::Closed);
    match phase {
        Phase::Opening | Phase::Unwanted | Phase::Running { .. } | Phase::Closing => {}
        Phase::Pending | Phase::Closed => unreachable!("a conversation ends once, once opened"),
    }
    // Its turns were counted as they were used; whatever its end counts beyond
    // them is counted now.
    let unaccounted = spend.saturating_sub(conversation.spent);
    let (run_id, asker) = (conversation.run, conversation.asker);
    facts.about(run_id.token());
    facts.push(Fact::Ended { run: run_id.token(), conversation: id.token(), end });
    conversations.retire(id);
    let run = runs.get_mut(run_id).expect("a run lives until its conversations have ended");
    run.spent = run.spent.saturating_add(unaccounted);
    run.conversations = run.conversations.checked_sub(1).expect("a run counts its conversations");
    // A sub-agent's end is its call's return.
    if let Some(asker) = asker {
        let call = calls.get_mut(asker).expect("a sub-agent's call lives until it has ended");
        let result = match &mut call.work {
            Work::Child(child) => agent::ended(child, end),
            Work::Landing(_) => unreachable!("a sub-agent serves a sub-agent's call"),
        };
        out.push(Request::Return { call: call.owner, result });
        retire_call(calls, alarms, conversations, asker);
        return;
    }
    let state = mem::replace(&mut run.state, State::Closed);
    run.state = match state {
        State::Working { reply_to, main } | State::Over { reply_to, main, exhausted: _ } => {
            assert!(main == id, "a conversation with no asker is main");
            answer(reply_to, ending(end, run.spent), out)
        }
        State::Winding { reply_to, ending } => answer(reply_to, finished(ending, run.spent), out),
        State::Preparing { .. } | State::Stopping { .. } | State::Closed => {
            unreachable!("a run's conversation ends only once the run has opened it")
        }
    };
    follow(runs, alarms, run_id);
}

pub(crate) fn deadline(domain: &mut Domain, id: Id<Run>, out: &mut Queue<Request>) {
    let Domain { runs, conversations, calls: _, alarms, facts } = domain;
    let run = runs.get_mut(id).expect("an alarm is cancelled before its run closes");
    facts.about(id.token());
    let failure = Failure::Budget(Exhausted::Time);
    let state = mem::replace(&mut run.state, State::Closed);
    run.state = match state {
        State::Preparing { reply_to, main, step: _ } => State::Stopping { reply_to, main, failure },
        State::Working { reply_to, main } => wind_down(conversations, reply_to, main, Ending::Failed(failure), out),
        // It was past its budget first.
        State::Over { reply_to, main, exhausted } => {
            wind_down(conversations, reply_to, main, Ending::Failed(Failure::Budget(exhausted)), out)
        }
        State::Stopping { .. } | State::Winding { .. } | State::Closed => {
            unreachable!("the deadline alarm runs only while a run prepares or works")
        }
    };
    follow(runs, alarms, id);
}

/// How deep the conversation `conversation` is, and for main what its run
/// found in its checkout (its guides, and its repositories with checks), for
/// their facts.
pub(crate) fn opened(
    runs: &Slab<Run>,
    conversations: &Slab<Conversation>,
    conversation: Token,
) -> (u32, Option<(u32, u32)>) {
    let conversation = conversations.get(Id::from_token(conversation)).expect("a conversation is told of as it opens");
    if conversation.depth > 0 {
        return (conversation.depth, None);
    }
    let run = runs.get(conversation.run).expect("a run outlives its conversations");
    (0, Some((run.found.guides.len(), run.found.checks.len())))
}

/// What a run's state implies, applied after every transition: whether its
/// deadline alarm runs, and whether it is retired.
fn follow(runs: &mut Slab<Run>, alarms: &mut Deadlines<Alarm>, id: Id<Run>) {
    let run = runs.get(id).expect("a run lives until it is retired");
    let (deadline, closed) = match &run.state {
        State::Preparing { .. } | State::Working { .. } | State::Over { .. } => (Some(run.deadline), false),
        State::Stopping { .. } | State::Winding { .. } => (None, false),
        State::Closed => (None, true),
    };
    let alarm = Alarm::Deadline { run: id };
    if let Some(at) = deadline {
        alarms.arm(alarm, at).expect("the alarm table has room for an alarm per run and per call");
    } else {
        alarms.cancel(alarm);
    }
    if closed {
        runs.retire(id);
    }
}

/// Whether a run in `state` may still finish: it works, or is over its budget
/// with main's turn in flight.
fn may_finish(state: &State) -> bool {
    match state {
        State::Working { .. } | State::Over { .. } => true,
        State::Preparing { .. } | State::Stopping { .. } | State::Winding { .. } | State::Closed => false,
    }
}

/// The landing of the call `id` has settled, as `settled` says: the run goes
/// on, or finishes.
fn settle(domain: &mut Domain, id: Id<Call>, settled: Settled, out: &mut Queue<Request>) {
    let Domain { runs, conversations, calls, alarms, facts: _ } = domain;
    if settled == Settled::Going {
        return;
    }
    let run_id = retire_call(calls, alarms, conversations, id);
    let run = runs.get_mut(run_id).expect("a run lives until its calls have returned");
    let state = mem::replace(&mut run.state, State::Closed);
    run.state = match settled {
        Settled::Pushed(change) => match state {
            State::Working { reply_to, main } | State::Over { reply_to, main, exhausted: _ } => {
                wind_down(conversations, reply_to, main, Ending::Accepted(Declared::Change(change)), out)
            }
            // The change is on the forge, whatever the run was winding down
            // for: main is closing already.
            State::Winding { reply_to, ending: _ } => {
                State::Winding { reply_to, ending: Ending::Accepted(Declared::Change(change)) }
            }
            State::Preparing { .. } | State::Stopping { .. } | State::Closed => {
                unreachable!("a run lands a change only once it has opened main, and before it answers")
            }
        },
        Settled::Refused => {
            run.rejected = run.rejected.saturating_add(1);
            match state {
                State::Over { reply_to, main, exhausted } => {
                    wind_down(conversations, reply_to, main, Ending::Failed(Failure::Budget(exhausted)), out)
                }
                state @ (State::Working { .. } | State::Winding { .. }) => state,
                State::Preparing { .. } | State::Stopping { .. } | State::Closed => {
                    unreachable!("a run lands a change only once it has opened main, and before it answers")
                }
            }
        }
        Settled::Stale => match state {
            State::Working { reply_to, main } | State::Over { reply_to, main, exhausted: _ } => {
                wind_down(conversations, reply_to, main, Ending::Failed(Failure::Stale), out)
            }
            state @ State::Winding { .. } => state,
            State::Preparing { .. } | State::Stopping { .. } | State::Closed => {
                unreachable!("a run lands a change only once it has opened main, and before it answers")
            }
        },
        Settled::Going | Settled::Cancelled => state,
    };
    follow(runs, alarms, run_id);
}

// Cell handlers: each takes the source state's data by value and returns the
// target state.

/// Asks for the look `step`, preparing.
fn look(
    run: &Run,
    id: Id<Run>,
    reply_to: ReplyTo,
    main: Id<Conversation>,
    step: Step,
    env: &Env<Limits>,
    out: &mut Queue<Request>,
) -> State {
    out.push(prepare::request(&run.charter, step, id.token(), env.now, &env.limits));
    State::Preparing { reply_to, main, step }
}

/// Preparing, the look `step` ended: the next one, or open main.
#[expect(clippy::too_many_arguments, reason = "a cell handler takes the fields it touches")]
fn prepared(
    run: &Run,
    conversations: &mut Slab<Conversation>,
    id: Id<Run>,
    reply_to: ReplyTo,
    main: Id<Conversation>,
    step: Step,
    env: &Env<Limits>,
    out: &mut Queue<Request>,
) -> State {
    match prepare::next(&run.charter, Some(step)) {
        Some(next) => look(run, id, reply_to, main, next, env, out),
        None => open(run, conversations, reply_to, main, env.now, out),
    }
}

/// Prepared: open main with what the run found, and the whole budget.
fn open(
    run: &Run,
    conversations: &mut Slab<Conversation>,
    reply_to: ReplyTo,
    main: Id<Conversation>,
    now: Time,
    out: &mut Queue<Request>,
) -> State {
    let conversation = conversations.get_mut(main).expect("main lives while its run prepares");
    let phase = mem::replace(&mut conversation.phase, Phase::Closed);
    conversation.phase = match phase {
        Phase::Pending => Phase::Opening,
        Phase::Opening | Phase::Unwanted | Phase::Running { .. } | Phase::Closing | Phase::Closed => {
            unreachable!("main is opened once, when its run has prepared")
        }
    };
    let opening = opening(&run.charter, &run.found, run.spent, run.deadline.saturating_since(now));
    out.push(Request::Open { conversation: main.token(), opening });
    State::Working { reply_to, main }
}

/// Stopping, the look in flight ended: main was never opened, and the run
/// answers.
fn stop(
    run: &Run,
    conversations: &mut Slab<Conversation>,
    reply_to: ReplyTo,
    main: Id<Conversation>,
    failure: Failure,
    out: &mut Queue<Request>,
) -> State {
    let conversation = conversations.get_mut(main).expect("main lives while its run prepares");
    let phase = mem::replace(&mut conversation.phase, Phase::Closed);
    match phase {
        Phase::Pending => {}
        Phase::Opening | Phase::Unwanted | Phase::Running { .. } | Phase::Closing | Phase::Closed => {
            unreachable!("main is not opened while its run prepares")
        }
    }
    conversations.retire(main);
    answer(reply_to, Answer::Failed { failure, spent: run.spent }, out)
}

/// Working, or over the budget's `over` part: main called `finish` as
/// `made`. A refused outcome is returned at once; an accepted verdict ends the
/// run; a change lands, by the call's deadline, and the run goes on meanwhile.
#[expect(clippy::too_many_arguments, reason = "a cell handler takes the fields it touches")]
fn finish(
    run: &mut Run,
    run_id: Id<Run>,
    conversations: &mut Slab<Conversation>,
    calls: &mut Calls,
    alarms: &mut Deadlines<Alarm>,
    reply_to: ReplyTo,
    main: Id<Conversation>,
    over: Option<Exhausted>,
    made: Asking,
    declared: Declared,
    env: &Env<Limits>,
    out: &mut Queue<Request>,
) -> State {
    let Asking { call, deadline } = made;
    let conversation = conversations.get(main).expect("main lives while its run works");
    assert!(conversation.calls == 0, "a finish is a write, which a conversation runs alone");
    let max = env.limits.outcome_bytes;
    let judged = match outcome::declared_cost(&declared) {
        Some(cost) if cost <= max => outcome::judge(&run.charter.outcome, &declared),
        Some(_) | None => Err(outcome::too_large(max)),
    };
    if let Err(problems) = judged {
        out.push(Request::Return { call, result: Returned::Rejected { problems } });
        run.rejected = run.rejected.saturating_add(1);
        return match over {
            None => State::Working { reply_to, main },
            Some(exhausted) => {
                wind_down(conversations, reply_to, main, Ending::Failed(Failure::Budget(exhausted)), out)
            }
        };
    }
    match declared {
        Declared::Verdict(verdict) => {
            out.push(Request::Return { call, result: Returned::Accepted });
            wind_down(conversations, reply_to, main, Ending::Accepted(Declared::Verdict(verdict)), out)
        }
        Declared::Change(change) => {
            if calls.is_full() {
                out.push(Request::Return { call, result: Returned::Busy });
                return match over {
                    None => State::Working { reply_to, main },
                    Some(exhausted) => State::Over { reply_to, main, exhausted },
                };
            }
            let work = Work::Landing(land::landing(change));
            let id = begin_call(calls, alarms, Call { run: run_id, conversation: main, owner: call, work }, deadline);
            match &mut calls.get_mut(id).expect("inserted above").work {
                Work::Landing(landing) => land::begin(landing, id, run, env, out),
                Work::Child(_) => unreachable!("inserted as a landing"),
            }
            let conversation = conversations.get_mut(main).expect("main lives while its run works");
            conversation.calls = conversation.calls.saturating_add(1);
            match over {
                None => State::Working { reply_to, main },
                Some(exhausted) => State::Over { reply_to, main, exhausted },
            }
        }
    }
}

/// A call a conversation made: its token for it, and its deadline.
struct Asking {
    call: Token,
    deadline: Time,
}

/// What a conversation asks for when it asks for a sub-agent.
struct Wanted {
    brief: Box<[u8]>,
    families: Families,
    llm: Option<Box<[u8]>>,
    share: Option<Spend>,
}

/// Working: the conversation `asker` asked for a sub-agent as `made`. Open
/// it, with no more time than to the call's deadline, or return why not.
#[expect(clippy::too_many_arguments, reason = "a cell handler takes the fields it touches")]
fn sub_agent(
    run: &mut Run,
    run_id: Id<Run>,
    conversations: &mut Slab<Conversation>,
    calls: &mut Calls,
    alarms: &mut Deadlines<Alarm>,
    asker: Id<Conversation>,
    made: Asking,
    wanted: Wanted,
    env: &Env<Limits>,
    out: &mut Queue<Request>,
) {
    let Asking { call, deadline } = made;
    let asking = conversations.get(asker).expect("a conversation lives until it has ended");
    let left = run.deadline.min(deadline).saturating_since(env.now);
    let means = Means { spent: run.spent, conversations: run.conversations, left };
    let planned = agent::plan(
        &run.charter,
        means,
        asking.families,
        asking.depth,
        wanted.families,
        wanted.llm.as_deref(),
        wanted.share,
        &env.limits,
    );
    let plan = match planned {
        Ok(plan) => plan,
        Err(refusal) => {
            out.push(Request::Return { call, result: Returned::Refused { refusal } });
            return;
        }
    };
    if conversations.is_full() || calls.is_full() {
        out.push(Request::Return { call, result: Returned::Busy });
        return;
    }
    // A call is stored before its child, which names it, and holds the child
    // once the child has a name too.
    let unnamed = Call { run: run_id, conversation: asker, owner: call, work: Work::Child(Child::Closed) };
    let id = begin_call(calls, alarms, unnamed, deadline);
    let child = Conversation {
        run: run_id,
        asker: Some(id),
        families: plan.families,
        depth: plan.depth,
        spent: Spend::ZERO,
        calls: 0,
        phase: Phase::Opening,
    };
    let child = conversations.insert(child).expect("checked for room above");
    calls.get_mut(id).expect("inserted above").work = Work::Child(agent::working(child));
    let asking = conversations.get_mut(asker).expect("a conversation lives until it has ended");
    asking.calls = asking.calls.saturating_add(1);
    run.conversations = run.conversations.saturating_add(1);
    let opening = Opening {
        system: prompt::child(&run.charter, &run.found, &wanted.brief, plan.families),
        prompt: copy_of(prompt::BEGIN),
        tools: plan.families.tools,
        checkout: run.charter.checkout.clone(),
        budget: plan.budget,
        finish: false,
        families: plan.families,
        llm: plan.llm,
    };
    out.push(Request::Open { conversation: child.token(), opening });
}

/// Stores `call`, there being room, and arms its deadline.
fn begin_call(calls: &mut Calls, alarms: &mut Deadlines<Alarm>, call: Call, deadline: Time) -> Id<Call> {
    let id = calls.insert(call);
    alarms.arm(Alarm::Call { call: id }, deadline).expect("the alarm table has room for an alarm per call");
    id
}

/// Stops what the call `id` is doing, for `why`; it returns once that has
/// settled.
fn stop_call(domain: &mut Domain, id: Id<Call>, why: Withdrawal, out: &mut Queue<Request>) {
    let call = domain.calls.get_mut(id).expect("a call lives until it returns, and its alarm with it");
    domain.facts.about(call.run.token());
    match &mut call.work {
        Work::Landing(landing) => land::withdraw(landing, id, why, out),
        Work::Child(child) => {
            if let Some(child) = agent::withdraw(child, why) {
                close(&mut domain.conversations, child, out);
            }
        }
    }
}

/// Retires the call `id`, which has returned: its alarm is cancelled, and its
/// conversation has a call fewer in flight. The run it is of.
fn retire_call(
    calls: &mut Calls,
    alarms: &mut Deadlines<Alarm>,
    conversations: &mut Slab<Conversation>,
    id: Id<Call>,
) -> Id<Run> {
    let call = calls.get(id).expect("a call lives until it returns");
    let (run, conversation) = (call.run, call.conversation);
    calls.retire(id);
    alarms.cancel(Alarm::Call { call: id });
    let conversation = conversations.get_mut(conversation).expect("a conversation outlives its calls");
    conversation.calls = conversation.calls.checked_sub(1).expect("a conversation counts its calls");
    run
}

/// Working, an ending decided: close main, and wait for it to end.
fn wind_down(
    conversations: &mut Slab<Conversation>,
    reply_to: ReplyTo,
    main: Id<Conversation>,
    ending: Ending,
    out: &mut Queue<Request>,
) -> State {
    close(conversations, main, out);
    State::Winding { reply_to, ending }
}

/// Closes the conversation `id`, which its owner closes once: at once, or
/// once it starts. A closing conversation withdraws its own calls.
fn close(conversations: &mut Slab<Conversation>, id: Id<Conversation>, out: &mut Queue<Request>) {
    let conversation = conversations.get_mut(id).expect("a conversation lives until it has ended");
    let phase = mem::replace(&mut conversation.phase, Phase::Closed);
    conversation.phase = match phase {
        // It is closed once it starts, or ends refused.
        Phase::Opening => Phase::Unwanted,
        Phase::Running { peer } => closing(peer, out),
        Phase::Pending | Phase::Unwanted | Phase::Closing | Phase::Closed => {
            unreachable!("a conversation is closed once it is opened, once")
        }
    };
}

/// Working, main yielded and may be nudged: tell it to carry on.
fn say(reply_to: ReplyTo, main: Id<Conversation>, peer: Token, text: Box<[u8]>, out: &mut Queue<Request>) -> State {
    out.push(Request::Say { peer, text });
    State::Working { reply_to, main }
}

fn closing(peer: Token, out: &mut Queue<Request>) -> Phase {
    out.push(Request::Close { peer });
    Phase::Closing
}

/// Answers the worker: the run is done.
fn answer(reply_to: ReplyTo, answer: Answer, out: &mut Queue<Request>) -> State {
    out.push(Request::Answer { to: reply_to, answer });
    State::Closed
}

// Helpers.

/// The opening of a run's main conversation, given what the run found in its
/// checkout, what it has spent and the time it has left. What it holds is
/// rendered from the charter or copied: the run keeps the charter (copy at
/// emission).
fn opening(charter: &Charter, found: &Found, spent: Spend, left: Duration) -> Opening {
    Opening {
        llm: charter.llm.clone(),
        system: prompt::system(charter, found),
        prompt: copy_of(prompt::BEGIN),
        tools: charter.grants.tools,
        checkout: charter.checkout.clone(),
        budget: charter.budget.remainder(spent, left),
        finish: true,
        families: Families::of(&charter.grants),
    }
}

/// The answer of a run whose main conversation ended on its own, the run
/// having spent `spent`.
fn ending(end: End, spent: Spend) -> Answer {
    match end {
        End::Busy => Answer::Refused(Refusal::Busy),
        End::Invalid => Answer::Refused(Refusal::Invalid(Invalid::Conversation)),
        End::Fault(fault) => Answer::Failed { failure: Failure::Model(fault), spent },
        End::Budget(exhausted) => Answer::Failed { failure: Failure::Budget(exhausted), spent },
        End::Closed => unreachable!("a conversation ends closed only once its run has closed it, winding down"),
    }
}

/// The answer of a run that wound down to `ending`, having spent `spent`.
fn finished(ending: Ending, spent: Spend) -> Answer {
    match ending {
        Ending::Accepted(outcome) => Answer::Accepted { outcome, spent },
        Ending::Failed(failure) => Answer::Failed { failure, spent },
    }
}

/// Counts a nudge for a run whose LLM stopped without finishing for `stop`,
/// or says how the run fails instead: when its nudges are used up, or no turn
/// is left in its budget for the LLM to carry on with.
fn nudge(run: &mut Run, stop: Stop, limits: &Limits) -> Result<(), Failure> {
    if run.nudges >= limits.nudges {
        return Err(unfinished(stop, run.nudges, run.rejected));
    }
    // Room to start a turn, as a conversation's ceilings have it: some
    // turns, input and output left.
    let spent = run.spent;
    let budget = &run.charter.budget;
    if spent.turns >= budget.turns {
        return Err(Failure::Budget(Exhausted::Turns));
    }
    if spent.input >= budget.input {
        return Err(Failure::Budget(Exhausted::Input));
    }
    if spent.output >= budget.output {
        return Err(Failure::Budget(Exhausted::Output));
    }
    run.nudges = run.nudges.saturating_add(1);
    Ok(())
}

/// How a run fails when its LLM stops without finishing, through `nudges`
/// nudges and `rejected` refused outcomes: as unfinished, or with the fault
/// its last stop shows.
fn unfinished(stop: Stop, nudges: u32, rejected: u32) -> Failure {
    match stop {
        Stop::EndTurn => Failure::Policy(Policy::Unfinished { nudges, rejected }),
        Stop::MaxTokens => Failure::Model(Fault::Truncated),
        Stop::Refusal => Failure::Model(Fault::Refused),
        Stop::NoCalls => Failure::Model(Fault::Malformed),
    }
}
