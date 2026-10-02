//! Runs: one agent instance, from its admission to its one answer.
//!
//! A `Start` admits a run, or refuses it at the entrance. An admitted run
//! opens its main conversation and works until how it ends is decided; then it
//! closes main, waits for main to end, and answers. The first ending decided
//! wins, and the run answers only once main has ended, so nothing it started
//! is still in flight.
//!
//! A run's transition table:
//!
//! ```text
//! state     event or alarm               next      emits
//! -         start, no room               -         answer: busy
//!           start, beyond the limits     -         answer: invalid
//!           start                        Working   admitted, open main
//! Working   main yielded                 Winding   close main: unfinished
//!           used, over the budget        Winding   close main: out of budget
//!           deadline                     Winding   close main: out of time
//!           cancel                       Winding   close main: cancelled
//!           main ended                   Closed    answer: how main ended
//! Winding   used, cancel                 Winding
//!           main ended                   Closed    answer: the ending decided
//! Closed    cancel                       Closed
//! ```
//!
//! and a conversation's, as the run keeps it:
//!
//! ```text
//! state     event or call                next
//! Opening   started                      Running
//!           closed by its run            Unwanted
//!           ended (refused)              Closed
//! Unwanted  started                      Closing   close it
//!           ended (refused)              Closed
//! Running   used                         Running
//!           closed by its run            Closing   close it
//!           ended                        Closed
//! Closing   yielded, used                Closing
//!           ended                        Closed
//! ```
//!
//! Every other cell is unreachable by the conversations' contract: `Started`
//! first unless refused, then events, then one `Ended`, which after a `Close`
//! comes once what was in flight has settled. A yield is decided in the step
//! it arrives, so a conversation never rests yielded. The deadline alarm runs
//! while a run works; it follows from the state, in one place ([`follow`]),
//! which also retires a run once it is Closed.

use core::mem;

use temper_lib::bytes::copy_of;
use temper_lib::{Deadlines, Duration, Env, Id, Queue, ReplyTo, Slab, Time, Token};

use crate::boundary::{Answer, End, Failure, Fault, Invalid, Opening, Policy, Refusal, Request, Stop};
use crate::budget::{Exhausted, Spend};
use crate::charter::{self, Charter};
use crate::limits::Limits;
use crate::model::Model;

/// The first user message of a main conversation, whose system text is the
/// brief.
pub(crate) const BEGIN: &[u8] = b"Begin the work your brief describes.";

#[derive(Debug)]
pub(crate) struct Run {
    charter: Charter,
    /// What its conversations have spent.
    spent: Spend,
    /// When its budget's time runs out.
    deadline: Time,
    state: State,
}

#[derive(Debug)]
enum State {
    /// Its main conversation is at work.
    Working { reply_to: ReplyTo, main: Id<Conversation> },
    /// It fails with `failure` once its main conversation has ended.
    Winding { reply_to: ReplyTo, failure: Failure },
    /// Terminal: holds nothing.
    Closed,
}

/// A conversation a run opened.
#[derive(Debug)]
pub(crate) struct Conversation {
    run: Id<Run>,
    /// What it has spent, by its `Used` so far.
    spent: Spend,
    phase: Phase,
}

#[derive(Debug)]
enum Phase {
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

/// A run's timers, named by what they are for.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum Alarm {
    /// Its budget's time runs out.
    Deadline { run: Id<Run> },
}

// Entry points, one per event or alarm: look the conversation or run up, take
// its state out, run the cell's handler, follow the run's new state.

pub(crate) fn start(
    model: &mut Model,
    env: &Env<Limits>,
    reply_to: ReplyTo,
    worker: Token,
    charter: Charter,
    out: &mut Queue<Request>,
) {
    let Model { runs, conversations, alarms } = model;
    if runs.is_full() || conversations.is_full() {
        out.push(Request::Answer { to: reply_to, answer: Answer::Refused(Refusal::Busy) });
        return;
    }
    if let Err(invalid) = charter::check(&charter, &env.limits) {
        out.push(Request::Answer { to: reply_to, answer: Answer::Refused(Refusal::Invalid(invalid)) });
        return;
    }
    let deadline = env.now.saturating_add(charter.budget.time);
    let opening = opening(&charter, Spend::ZERO, deadline.saturating_since(env.now));
    // A run is stored before its main conversation, which names it, and starts
    // work once main has a name too.
    let run = Run { charter, spent: Spend::ZERO, deadline, state: State::Closed };
    let id = runs.insert(run).expect("checked for room above");
    let conversation = Conversation { run: id, spent: Spend::ZERO, phase: Phase::Opening };
    let main = conversations.insert(conversation).expect("checked for room above");
    runs.get_mut(id).expect("inserted above").state = State::Working { reply_to, main };
    out.push(Request::Admitted { worker, run: id.token() });
    out.push(Request::Open { conversation: main.token(), opening });
    follow(runs, alarms, id);
}

pub(crate) fn cancel(model: &mut Model, run: Token, out: &mut Queue<Request>) {
    let Model { runs, conversations, alarms } = model;
    let id = Id::<Run>::from_token(run);
    // A cancel travels down, so it may name a run that has answered and gone.
    let Some(run) = runs.get_mut(id) else {
        return;
    };
    let state = mem::replace(&mut run.state, State::Closed);
    run.state = match state {
        State::Working { reply_to, main } => wind_down(conversations, reply_to, main, Failure::Cancelled, out),
        // How the run ends is decided already.
        State::Winding { reply_to, failure } => State::Winding { reply_to, failure },
        // It answered in this iteration, and is retired already.
        State::Closed => return,
    };
    follow(runs, alarms, id);
}

pub(crate) fn started(model: &mut Model, conversation: Token, peer: Token, out: &mut Queue<Request>) {
    let id = Id::<Conversation>::from_token(conversation);
    let conversation = model.conversations.get_mut(id).expect("a conversation lives until it has ended");
    let phase = mem::replace(&mut conversation.phase, Phase::Closed);
    conversation.phase = match phase {
        Phase::Opening => Phase::Running { peer },
        Phase::Unwanted => close(peer, out),
        Phase::Running { .. } | Phase::Closing | Phase::Closed => {
            unreachable!("a conversation starts once, before anything else")
        }
    };
}

pub(crate) fn yielded(model: &mut Model, conversation: Token, stop: Stop, out: &mut Queue<Request>) {
    let Model { runs, conversations, alarms } = model;
    let id = Id::<Conversation>::from_token(conversation);
    let conversation = conversations.get(id).expect("a conversation lives until it has ended");
    match &conversation.phase {
        Phase::Running { .. } => {}
        // The yield crossed the run's close: there is nothing left to decide.
        Phase::Closing => return,
        Phase::Opening | Phase::Unwanted | Phase::Closed => {
            unreachable!("a conversation yields only between starting and ending")
        }
    }
    let run_id = conversation.run;
    let run = runs.get_mut(run_id).expect("a run lives until its conversations have ended");
    let state = mem::replace(&mut run.state, State::Closed);
    run.state = match state {
        State::Working { reply_to, main } => {
            assert!(main == id, "a run's only conversation is its main one");
            wind_down(conversations, reply_to, main, unfinished(stop, 0), out)
        }
        State::Winding { .. } | State::Closed => {
            unreachable!("a run closes its conversation when it winds down, and it yields no more")
        }
    };
    follow(runs, alarms, run_id);
}

pub(crate) fn used(model: &mut Model, conversation: Token, spend: Spend, out: &mut Queue<Request>) {
    let Model { runs, conversations, alarms } = model;
    let id = Id::<Conversation>::from_token(conversation);
    let conversation = conversations.get_mut(id).expect("a conversation lives until it has ended");
    match &conversation.phase {
        Phase::Running { .. } | Phase::Closing => {}
        Phase::Opening | Phase::Unwanted | Phase::Closed => {
            unreachable!("a conversation spends only between starting and ending")
        }
    }
    conversation.spent = conversation.spent.saturating_add(spend);
    let run_id = conversation.run;
    let run = runs.get_mut(run_id).expect("a run lives until its conversations have ended");
    run.spent = run.spent.saturating_add(spend);
    let state = mem::replace(&mut run.state, State::Closed);
    run.state = match state {
        State::Working { reply_to, main } => match run.charter.budget.overspent(run.spent) {
            Some(exhausted) => wind_down(conversations, reply_to, main, Failure::Budget(exhausted), out),
            None => State::Working { reply_to, main },
        },
        // Spent all the same, and counted in the answer.
        State::Winding { reply_to, failure } => State::Winding { reply_to, failure },
        State::Closed => unreachable!("a run lives until its conversations have ended"),
    };
    follow(runs, alarms, run_id);
}

pub(crate) fn ended(model: &mut Model, conversation: Token, end: End, spend: Spend, out: &mut Queue<Request>) {
    let Model { runs, conversations, alarms } = model;
    let id = Id::<Conversation>::from_token(conversation);
    let conversation = conversations.get_mut(id).expect("a conversation lives until it has ended");
    let phase = mem::replace(&mut conversation.phase, Phase::Closed);
    match phase {
        Phase::Opening | Phase::Unwanted | Phase::Running { .. } | Phase::Closing => {}
        Phase::Closed => unreachable!("a conversation ends once"),
    }
    // Its turns were counted as they were used; whatever its end counts beyond
    // them is counted now.
    let unaccounted = spend.saturating_sub(conversation.spent);
    let run_id = conversation.run;
    conversations.retire(id);
    let run = runs.get_mut(run_id).expect("a run lives until its conversations have ended");
    run.spent = run.spent.saturating_add(unaccounted);
    let state = mem::replace(&mut run.state, State::Closed);
    run.state = match state {
        State::Working { reply_to, main } => {
            assert!(main == id, "a run's only conversation is its main one");
            answer(reply_to, ending(end, run.spent), out)
        }
        State::Winding { reply_to, failure } => answer(reply_to, Answer::Failed { failure, spent: run.spent }, out),
        State::Closed => unreachable!("a run lives until its conversations have ended"),
    };
    follow(runs, alarms, run_id);
}

pub(crate) fn deadline(model: &mut Model, id: Id<Run>, out: &mut Queue<Request>) {
    let Model { runs, conversations, alarms } = model;
    let run = runs.get_mut(id).expect("an alarm is cancelled before its run closes");
    let state = mem::replace(&mut run.state, State::Closed);
    run.state = match state {
        State::Working { reply_to, main } => {
            wind_down(conversations, reply_to, main, Failure::Budget(Exhausted::Time), out)
        }
        State::Winding { .. } | State::Closed => unreachable!("the deadline alarm runs only while a run works"),
    };
    follow(runs, alarms, id);
}

/// What a run's state implies, applied after every transition: whether its
/// deadline alarm runs, and whether it is retired.
fn follow(runs: &mut Slab<Run>, alarms: &mut Deadlines<Alarm>, id: Id<Run>) {
    let run = runs.get(id).expect("a run lives until it is retired");
    let (deadline, closed) = match &run.state {
        State::Working { .. } => (Some(run.deadline), false),
        State::Winding { .. } => (None, false),
        State::Closed => (None, true),
    };
    let alarm = Alarm::Deadline { run: id };
    if let Some(at) = deadline {
        alarms.arm(alarm, at).expect("the alarm table has room for one alarm per run");
    } else {
        alarms.cancel(alarm);
    }
    if closed {
        runs.retire(id);
    }
}

// Cell handlers: each takes the source state's data by value and returns the
// target state.

/// Working, an ending decided: close main, and wait for it to end.
fn wind_down(
    conversations: &mut Slab<Conversation>,
    reply_to: ReplyTo,
    main: Id<Conversation>,
    failure: Failure,
    out: &mut Queue<Request>,
) -> State {
    let conversation = conversations.get_mut(main).expect("main lives while its run works");
    let phase = mem::replace(&mut conversation.phase, Phase::Closed);
    conversation.phase = match phase {
        // It is closed once it starts, or ends refused.
        Phase::Opening => Phase::Unwanted,
        Phase::Running { peer } => close(peer, out),
        Phase::Unwanted | Phase::Closing | Phase::Closed => unreachable!("a run closes main once, winding down"),
    };
    State::Winding { reply_to, failure }
}

fn close(peer: Token, out: &mut Queue<Request>) -> Phase {
    out.push(Request::Close { peer });
    Phase::Closing
}

/// Answers the worker: the run is done.
fn answer(reply_to: ReplyTo, answer: Answer, out: &mut Queue<Request>) -> State {
    out.push(Request::Answer { to: reply_to, answer });
    State::Closed
}

// Helpers.

/// The opening of a run's main conversation, given what the run has spent and
/// the time it has left. It is a copy of the charter's parts: the run keeps
/// the charter (copy at emission).
fn opening(charter: &Charter, spent: Spend, left: Duration) -> Opening {
    Opening {
        llm: charter.llm.clone(),
        system: copy_of(&charter.brief),
        prompt: copy_of(BEGIN),
        tools: charter.grants.tools,
        checkout: charter.checkout.clone(),
        budget: charter.budget.remainder(spent, left),
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

/// How a run fails when its LLM stops without finishing, through `nudges`
/// nudges: as unfinished, or with the fault its last stop shows.
fn unfinished(stop: Stop, nudges: u32) -> Failure {
    match stop {
        Stop::EndTurn => Failure::Policy(Policy::Unfinished { nudges }),
        Stop::MaxTokens => Failure::Model(Fault::Truncated),
        Stop::Refusal => Failure::Model(Fault::Refused),
        Stop::NoCalls => Failure::Model(Fault::Malformed),
    }
}
