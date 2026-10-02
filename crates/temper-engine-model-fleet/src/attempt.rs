//! The attempts the fleet tracks (engine-model.md, 4.2 and section 8): each
//! the parent's claim on a run, from its start or its adoption to its one
//! answer, or one a worker hosts that the parent has not claimed.
//!
//! A run's attempts share its workstream (the workstream is the item, and a
//! run is named by its item), so no attempt of a run is placed while a worker
//! may host another: an attempt replaced, cancelled or found unclaimed holds
//! its run's next one back until its worker answers it, or past the grace
//! of its worker's lost channel, when the worker has cancelled it itself.
//! What a worker sends for an attempt that is not the parent's live claim is
//! dropped (attempts are fenced): only a cancelled attempt's answer still
//! ends its call.
//!
//! An attempt's transition table. "On" is on the channel of a worker in
//! contact, which it takes a slot of; "adrift", its worker's channel lost,
//! kept until the grace passes. A listing is a hello's, "answered" when the
//! worker holds the attempt's answer, which follows the hello.
//!
//! ```text
//! state      event or alarm     next             emits
//! (none)     start              Waiting          (a newer claim replaces its run's claim)
//!            adopt              Adopted          (likewise)
//!            listed, its run    Fenced on        cancel, unless answered
//!              claimed
//!            listed             Stray on
//! Waiting    placed (resume)    Claimed on       assign; placed
//!            listed             Claimed on       placed (it is hosted already)
//!            cancel             (gone)           withdrawn: cancelled
//!            replaced           (gone)           withdrawn: replaced
//! Adopted    listed             Claimed on       placed
//!            cancel             Cancelled adrift
//!            replaced           Fenced adrift    withdrawn: replaced
//!            answer             (gone)           acknowledge; answered
//!            grace              (gone)           lost
//! Claimed    listed             Claimed on       (moved)
//!            cancel             Cancelled        cancel, if on
//!            replaced           Fenced           cancel, if on; withdrawn: replaced
//!            answer             (gone)           acknowledge; answered
//!            channel lost       Claimed adrift
//!            grace (adrift)     (gone)           lost
//! Cancelled  listed             Cancelled on     cancel, unless answered
//!            cancel             Cancelled
//!            replaced           Fenced           withdrawn: replaced
//!            answer             (gone)           acknowledge; answered
//!            channel lost       Cancelled adrift
//!            grace (adrift)     (gone)           lost
//! Stray      listed             Stray on         (moved)
//!            adopt              Claimed          placed
//!            cancel             Fenced           cancel, if on
//!            answer             Kept             acknowledge
//!            channel lost       Stray adrift
//!            its deadline       Fenced           cancel, if on
//! Kept       adopt              (gone)           answered
//!            cancel             (gone)           drop the answer
//!            its deadline       (gone)           drop the answer
//!            listed             Kept             (counted: its worker forgot it)
//!            answer             Kept             acknowledge; drop (a duplicate)
//! Fenced     listed             Fenced on        cancel, unless answered
//!            answer             (gone)           acknowledge; drop
//!            cancel             Fenced
//!            channel lost       Fenced adrift
//!            grace (adrift)     (gone)
//! ```
//!
//! An answer for an attempt waiting to be placed, or one the fleet does not
//! know (answered already, or never made), is acknowledged and dropped. A
//! stray's deadline is the grace from its first listing, which comes no later
//! than the grace of its worker's channel lost since. Only the parent's
//! claims (Waiting, Adopted, Claimed, Cancelled) are replaced: a stray of the
//! run is left to its deadline.
//!
//! What a state implies (the slot it takes, whether its run's next attempt
//! waits for it, its place in the queue for a slot, whether it is its run's
//! claim, and its alarm) is derived from it in one place after every
//! transition ([`follow`]).

use alloc::boxed::Box;
use core::mem;

use temper_lib::{Env, Id, Queue, ReplyTo, Slab, Time, Token};

use crate::boundary::{Answer, Refusal, Request, Withdrawal};
use crate::channel::{self, Channel};
use crate::facts::{Fact, Facts};
use crate::limits::Limits;
use crate::model::Model;

/// An attempt the fleet tracks.
#[derive(Debug)]
pub(crate) struct Attempt {
    /// Its run's name, and its own, as the parent and the workers give them.
    pub(crate) run: Token,
    pub(crate) token: Token,
    pub(crate) state: State,
}

/// What the fleet knows of a run: its claim, and how many of its attempts a
/// worker may host.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Run {
    /// The attempt the parent claims it by, whose call is open.
    pub(crate) claim: Option<Id<Attempt>>,
    /// Its attempts a worker may host. Its claim waits for a slot until none
    /// is.
    pub(crate) held: u32,
}

#[derive(Debug)]
pub(crate) enum State {
    /// Started: waits for a slot, and for no worker to host another attempt
    /// of its run. `serial` is its place in the queue.
    Waiting { to: ReplyTo, workstream: Box<[u8]>, serial: u64 },
    /// Adopted after a restart, no worker having listed it yet: until the
    /// grace passes.
    Adopted { to: ReplyTo, until: Time },
    /// The parent's live claim, on a worker or adrift.
    Claimed { to: ReplyTo, at: Where },
    /// Cancelled by the parent: its answer still ends the call.
    Cancelled { to: ReplyTo, at: Where },
    /// Listed by a worker, and not claimed: it waits to be adopted until
    /// `until`.
    Stray { at: Where, until: Time },
    /// A stray's answer, acknowledged, kept for its adoption until `until`.
    Kept { answer: Answer, payload: Token, until: Time },
    /// Cancelled for good, replaced or not adopted in time: whatever comes
    /// for it is dropped. It holds its run's next attempt back while a
    /// worker may host it.
    Fenced { at: Where },
    /// Terminal: holds nothing.
    Closed,
}

/// Where an attempt out on a worker is.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Where {
    /// On the worker of this channel, taking one of its slots.
    On(Id<Channel>),
    /// Its worker's channel was lost: kept until the grace passes.
    Adrift { until: Time },
}

/// What a state implies.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Implied {
    /// The channel whose slot it takes.
    on: Option<Id<Channel>>,
    /// Whether a worker may host it.
    held: bool,
    /// Its place in the queue for a slot.
    queued: Option<u64>,
    /// Whether it is its run's claim.
    claim: bool,
    /// When its alarm falls due.
    deadline: Option<Time>,
    closed: bool,
}

/// What `state` implies.
pub(crate) fn implied(state: &State) -> Implied {
    let none = Implied { on: None, held: false, queued: None, claim: false, deadline: None, closed: false };
    match state {
        State::Waiting { serial, .. } => Implied { queued: Some(*serial), claim: true, ..none },
        State::Adopted { until, .. } => Implied { held: true, claim: true, deadline: Some(*until), ..none },
        State::Claimed { at, .. } | State::Cancelled { at, .. } => {
            Implied { on: on(*at), held: true, claim: true, deadline: until(*at), ..none }
        }
        State::Stray { at, until } => Implied { on: on(*at), held: true, deadline: Some(*until), ..none },
        State::Kept { until, .. } => Implied { deadline: Some(*until), ..none },
        State::Fenced { at } => Implied { on: on(*at), held: true, deadline: until(*at), ..none },
        State::Closed => Implied { closed: true, ..none },
    }
}

fn on(at: Where) -> Option<Id<Channel>> {
    match at {
        Where::On(channel) => Some(channel),
        Where::Adrift { .. } => None,
    }
}

fn until(at: Where) -> Option<Time> {
    match at {
        Where::On(_) => None,
        Where::Adrift { until } => Some(until),
    }
}

/// What nothing implies: an attempt not tracked yet.
const UNTRACKED: Implied = Implied { on: None, held: false, queued: None, claim: false, deadline: None, closed: false };

/// The names a request about an attempt carries.
#[derive(Clone, Copy, Debug)]
struct Names {
    run: Token,
    attempt: Token,
}

// Entry points: one per event, alarm or resume that concerns an attempt.

/// Start: refused at the entrance, or queued for a slot, its run's claim
/// replaced.
pub(crate) fn start(
    model: &mut Model,
    env: &Env<Limits>,
    to: ReplyTo,
    run: Token,
    attempt: Token,
    workstream: Box<[u8]>,
    out: &mut Queue<Request>,
) {
    let refusal = if workstream.is_empty() || workstream.len() > channel::bytes(env.limits.workstream_bytes) {
        Some(Refusal::Workstream)
    } else if model.names.contains_key(&(run, attempt)) {
        Some(Refusal::Duplicate)
    } else if model.attempts.is_full() {
        Some(Refusal::Busy)
    } else {
        None
    };
    if let Some(refusal) = refusal {
        refuse(model, to, Names { run, attempt }, refusal, out);
        return;
    }
    replace(model, run, out);
    let serial = model.serial;
    model.serial = serial.checked_add(1).expect("a u64 counts every start");
    insert(model, run, attempt, State::Waiting { to, workstream, serial });
}

/// Adopt: a stray claimed, a kept answer handed over, or a claim adrift
/// until a worker lists it; refused if the fleet has it already, and lost if
/// it was fenced off.
pub(crate) fn adopt(
    model: &mut Model,
    env: &Env<Limits>,
    to: ReplyTo,
    run: Token,
    attempt: Token,
    out: &mut Queue<Request>,
) {
    let names = Names { run, attempt };
    let Some(&id) = model.names.get(&(run, attempt)) else {
        if model.attempts.is_full() {
            refuse(model, to, names, Refusal::Busy, out);
            return;
        }
        replace(model, run, out);
        let until = env.now.saturating_add(env.limits.grace);
        insert(model, run, attempt, State::Adopted { to, until });
        return;
    };
    let entry = model.attempts.get(id).expect("a named attempt is tracked");
    match &entry.state {
        State::Stray { .. } | State::Kept { .. } => {}
        State::Waiting { .. } | State::Adopted { .. } | State::Claimed { .. } | State::Cancelled { .. } => {
            refuse(model, to, names, Refusal::Duplicate, out);
            return;
        }
        // Cancelled for good: as far as the parent can tell, lost.
        State::Fenced { .. } => {
            model.facts.push(Fact::PresumedLost);
            out.push(Request::Lost { to, run, attempt });
            return;
        }
        State::Closed => unreachable!("a closed attempt is no longer named"),
    }
    replace(model, run, out);
    let entry = model.attempts.get_mut(id).expect("looked up above");
    let before = implied(&entry.state);
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Stray { at, until: _ } => located(to, at, names, &mut model.facts, out),
        State::Kept { answer, payload, until: _ } => handed(to, answer, payload, names, &mut model.facts, out),
        State::Waiting { .. }
        | State::Adopted { .. }
        | State::Claimed { .. }
        | State::Cancelled { .. }
        | State::Fenced { .. }
        | State::Closed => unreachable!("only a stray or a kept answer is adopted, as matched above"),
    };
    follow(model, id, before);
}

/// Cancel, from the parent. An attempt the fleet no longer tracks has
/// answered, been lost or withdrawn, or was never made: nothing to cancel.
pub(crate) fn cancel(model: &mut Model, run: Token, attempt: Token, out: &mut Queue<Request>) {
    let Some(&id) = model.names.get(&(run, attempt)) else {
        return;
    };
    let names = Names { run, attempt };
    let entry = model.attempts.get_mut(id).expect("a named attempt is tracked");
    let before = implied(&entry.state);
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Waiting { to, workstream: _, serial: _ } => withdrawn(to, Withdrawal::Cancelled, names, out),
        State::Adopted { to, until } => State::Cancelled { to, at: Where::Adrift { until } },
        State::Claimed { to, at } => cancelled(to, at, names, &model.channels, out),
        State::Cancelled { to, at } => State::Cancelled { to, at },
        State::Stray { at, until: _ } => fenced(at, names, &model.channels, &mut model.facts, out),
        State::Kept { answer: _, payload, until: _ } => dropped(payload, out),
        State::Fenced { at } => State::Fenced { at },
        State::Closed => unreachable!("a closed attempt is no longer named"),
    };
    follow(model, id, before);
}

/// Placed, from the ready list: the waiting attempt `id` is assigned to the
/// worker of `channel`, which has a free slot.
pub(crate) fn place(
    model: &mut Model,
    env: &Env<Limits>,
    id: Id<Attempt>,
    channel: Id<Channel>,
    out: &mut Queue<Request>,
) {
    let entry = model.attempts.get_mut(id).expect("a queued attempt is tracked");
    let names = Names { run: entry.run, attempt: entry.token };
    let before = implied(&entry.state);
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Waiting { to, workstream, serial: _ } => {
            assigned(to, workstream, channel, names, env, &mut model.channels, &mut model.facts, out)
        }
        State::Adopted { .. }
        | State::Claimed { .. }
        | State::Cancelled { .. }
        | State::Stray { .. }
        | State::Kept { .. }
        | State::Fenced { .. }
        | State::Closed => unreachable!("only a waiting attempt is queued for a slot"),
    };
    follow(model, id, before);
}

/// Listed by the hello of the worker of `channel`: what it hosts is kept,
/// cancelled again, or found.
pub(crate) fn listed(
    model: &mut Model,
    env: &Env<Limits>,
    channel: Id<Channel>,
    run: Token,
    attempt: Token,
    answered: bool,
    out: &mut Queue<Request>,
) {
    let names = Names { run, attempt };
    let Some(&id) = model.names.get(&(run, attempt)) else {
        found(model, env, channel, names, answered, out);
        return;
    };
    let entry = model.attempts.get_mut(id).expect("a named attempt is tracked");
    let before = implied(&entry.state);
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Waiting { to, workstream, serial: _ } => {
            hosted(to, workstream, channel, names, env, &mut model.channels, &mut model.facts, out)
        }
        State::Adopted { to, until: _ } => located(to, Where::On(channel), names, &mut model.facts, out),
        State::Claimed { to, at } => moved(to, at, channel, &mut model.facts),
        State::Cancelled { to, at: _ } => {
            again(Where::On(channel), answered, names, &model.channels, out);
            State::Cancelled { to, at: Where::On(channel) }
        }
        State::Stray { at: _, until } => State::Stray { at: Where::On(channel), until },
        State::Kept { answer, payload, until } => {
            model.facts.push(Fact::Dropped);
            State::Kept { answer, payload, until }
        }
        State::Fenced { at: _ } => {
            again(Where::On(channel), answered, names, &model.channels, out);
            State::Fenced { at: Where::On(channel) }
        }
        State::Closed => unreachable!("a closed attempt is no longer named"),
    };
    follow(model, id, before);
}

/// An answer, on the channel `channel`: acknowledged in any case, and
/// passed to the parent once, if the attempt is its claim.
pub(crate) fn answer(
    model: &mut Model,
    channel: Token,
    run: Token,
    attempt: Token,
    answer: Answer,
    payload: Token,
    out: &mut Queue<Request>,
) {
    out.push(Request::Acknowledge { channel, run, attempt });
    match answer {
        Answer::Busy => channel::drain(model, channel),
        Answer::Ended | Answer::Parked | Answer::Failed | Answer::Invalid => {}
    }
    let names = Names { run, attempt };
    let Some(&id) = model.names.get(&(run, attempt)) else {
        // The parent has it already, or it was lost, withdrawn or never made.
        duplicate(payload, &mut model.facts, out);
        return;
    };
    let entry = model.attempts.get_mut(id).expect("a named attempt is tracked");
    let before = implied(&entry.state);
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Waiting { to, workstream, serial } => {
            duplicate(payload, &mut model.facts, out);
            State::Waiting { to, workstream, serial }
        }
        State::Adopted { to, until: _ } | State::Claimed { to, at: _ } | State::Cancelled { to, at: _ } => {
            handed(to, answer, payload, names, &mut model.facts, out)
        }
        State::Stray { at: _, until } => State::Kept { answer, payload, until },
        State::Kept { answer: kept, payload: held, until } => {
            duplicate(payload, &mut model.facts, out);
            State::Kept { answer: kept, payload: held, until }
        }
        State::Fenced { at: _ } => {
            duplicate(payload, &mut model.facts, out);
            State::Closed
        }
        State::Closed => unreachable!("a closed attempt is no longer named"),
    };
    follow(model, id, before);
}

/// Its worker's channel was lost: the attempt `id` is kept until `until`.
pub(crate) fn adrift(model: &mut Model, id: Id<Attempt>, until: Time) {
    let entry = model.attempts.get_mut(id).expect("an attempt on a worker is tracked");
    let before = implied(&entry.state);
    let state = mem::replace(&mut entry.state, State::Closed);
    let at = Where::Adrift { until };
    entry.state = match state {
        State::Claimed { to, at: _ } => State::Claimed { to, at },
        State::Cancelled { to, at: _ } => State::Cancelled { to, at },
        State::Stray { at: _, until } => State::Stray { at, until },
        State::Fenced { at: _ } => State::Fenced { at },
        State::Waiting { .. } | State::Adopted { .. } | State::Kept { .. } | State::Closed => {
            unreachable!("only an attempt on a worker takes its slot")
        }
    };
    follow(model, id, before);
}

/// The alarm of the attempt `id`: the grace of its lost channel, or of its
/// adoption, passed; or a stray's or a kept answer's deadline.
pub(crate) fn expire(model: &mut Model, id: Id<Attempt>, out: &mut Queue<Request>) {
    let entry = model.attempts.get_mut(id).expect("an alarm names a tracked attempt");
    let names = Names { run: entry.run, attempt: entry.token };
    let before = implied(&entry.state);
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Adopted { to, until: _ }
        | State::Claimed { to, at: Where::Adrift { .. } }
        | State::Cancelled { to, at: Where::Adrift { .. } } => presumed(to, names, &mut model.facts, out),
        State::Stray { at, until: _ } => fenced(at, names, &model.channels, &mut model.facts, out),
        State::Kept { answer: _, payload, until: _ } => dropped(payload, out),
        State::Fenced { at: Where::Adrift { .. } } => State::Closed,
        State::Waiting { .. }
        | State::Claimed { at: Where::On(_), .. }
        | State::Cancelled { at: Where::On(_), .. }
        | State::Fenced { at: Where::On(_) }
        | State::Closed => unreachable!("no alarm runs in this state"),
    };
    follow(model, id, before);
}

/// Resumes the ready list: the first attempt in the queue whose run no
/// worker hosts another of, for which a worker has a free slot, is placed.
/// With none, placement waits for something to change.
pub(crate) fn resume(model: &mut Model, env: &Env<Limits>, out: &mut Queue<Request>) {
    let mut chosen = None;
    for (_, &id) in &model.waiting {
        let entry = model.attempts.get(id).expect("a queued attempt is tracked");
        let held = match model.runs.get(&entry.run) {
            Some(run) => run.held,
            None => unreachable!("a queued attempt is its run's claim"),
        };
        if held > 0 {
            continue;
        }
        let workstream = match &entry.state {
            State::Waiting { workstream, .. } => workstream,
            State::Adopted { .. }
            | State::Claimed { .. }
            | State::Cancelled { .. }
            | State::Stray { .. }
            | State::Kept { .. }
            | State::Fenced { .. }
            | State::Closed => unreachable!("only a waiting attempt is queued for a slot"),
        };
        if let Some(channel) = channel::choose(model, workstream) {
            chosen = Some((id, channel));
            break;
        }
    }
    match chosen {
        Some((id, channel)) => place(model, env, id, channel, out),
        None => model.placing = false,
    }
}

// Cell handlers: each takes the source state's data by value and returns the
// target state.

/// Refused at the entrance.
fn refuse(model: &mut Model, to: ReplyTo, names: Names, refusal: Refusal, out: &mut Queue<Request>) {
    model.facts.push(Fact::Refused { refusal });
    out.push(Request::Refused { to, run: names.run, attempt: names.attempt, refusal });
}

/// A newer claim of `run`: its claim, if it has one, is withdrawn, and fenced
/// if a worker may host it.
fn replace(model: &mut Model, run: Token, out: &mut Queue<Request>) {
    let claim = match model.runs.get(&run) {
        Some(entry) => entry.claim,
        None => None,
    };
    let Some(id) = claim else {
        return;
    };
    let entry = model.attempts.get_mut(id).expect("a run's claim is tracked");
    let names = Names { run, attempt: entry.token };
    let before = implied(&entry.state);
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Waiting { to, workstream: _, serial: _ } => withdrawn(to, Withdrawal::Replaced, names, out),
        State::Adopted { to, until } => superseded(to, Where::Adrift { until }, names, out),
        State::Claimed { to, at } => {
            again(at, false, names, &model.channels, out);
            superseded(to, at, names, out)
        }
        State::Cancelled { to, at } => superseded(to, at, names, out),
        State::Stray { .. } | State::Kept { .. } | State::Fenced { .. } | State::Closed => {
            unreachable!("a run's claim is the parent's")
        }
    };
    follow(model, id, before);
}

/// Waiting, cancelled or replaced: its call ends, and nothing of it is left.
fn withdrawn(to: ReplyTo, withdrawal: Withdrawal, names: Names, out: &mut Queue<Request>) -> State {
    out.push(Request::Withdrawn { to, run: names.run, attempt: names.attempt, withdrawal });
    State::Closed
}

/// A claim on a worker, or maybe on one, replaced: its call ends, and it is
/// fenced off while a worker may host it.
fn superseded(to: ReplyTo, at: Where, names: Names, out: &mut Queue<Request>) -> State {
    out.push(Request::Withdrawn { to, run: names.run, attempt: names.attempt, withdrawal: Withdrawal::Replaced });
    State::Fenced { at }
}

/// Claimed, cancel: the cancel goes to its worker now, or once a hello
/// lists it.
fn cancelled(to: ReplyTo, at: Where, names: Names, channels: &Slab<Channel>, out: &mut Queue<Request>) -> State {
    again(at, false, names, channels, out);
    State::Cancelled { to, at }
}

/// A stray cancelled, or not adopted in time: fenced off, and cancelled on
/// its worker.
fn fenced(at: Where, names: Names, channels: &Slab<Channel>, facts: &mut Facts, out: &mut Queue<Request>) -> State {
    facts.push(Fact::Fenced);
    again(at, false, names, channels, out);
    State::Fenced { at }
}

/// Sends the attempt's cancel to its worker, if it is on one and has not
/// answered.
fn again(at: Where, answered: bool, names: Names, channels: &Slab<Channel>, out: &mut Queue<Request>) {
    match at {
        Where::On(channel) if !answered => {
            let channel = channel::token(channels, channel);
            out.push(Request::Cancel { channel, run: names.run, attempt: names.attempt });
        }
        Where::On(_) | Where::Adrift { .. } => {}
    }
}

/// A kept answer no one adopts: the parent forgets it.
fn dropped(payload: Token, out: &mut Queue<Request>) -> State {
    out.push(Request::Drop { payload });
    State::Closed
}

/// An answer the parent does not take: it forgets it.
fn duplicate(payload: Token, facts: &mut Facts, out: &mut Queue<Request>) {
    facts.push(Fact::Duplicate);
    out.push(Request::Drop { payload });
}

/// The answer goes to the parent, ending its call.
fn handed(
    to: ReplyTo,
    answer: Answer,
    payload: Token,
    names: Names,
    facts: &mut Facts,
    out: &mut Queue<Request>,
) -> State {
    facts.push(Fact::Answered { answer });
    out.push(Request::Answered { to, run: names.run, attempt: names.attempt, answer, payload });
    State::Closed
}

/// Past the grace, no worker hosting it in contact: presumed lost.
fn presumed(to: ReplyTo, names: Names, facts: &mut Facts, out: &mut Queue<Request>) -> State {
    facts.push(Fact::PresumedLost);
    out.push(Request::Lost { to, run: names.run, attempt: names.attempt });
    State::Closed
}

/// An adopted attempt found, or a stray adopted: the parent's claim from now
/// on, which the parent hears is on a worker.
fn located(to: ReplyTo, at: Where, names: Names, facts: &mut Facts, out: &mut Queue<Request>) -> State {
    facts.push(Fact::Found);
    out.push(Request::Placed { run: names.run, attempt: names.attempt });
    State::Claimed { to, at }
}

/// A claim listed again: on its worker's channel from now on.
fn moved(to: ReplyTo, at: Where, channel: Id<Channel>, facts: &mut Facts) -> State {
    match at {
        Where::On(_) => {}
        Where::Adrift { .. } => facts.push(Fact::Found),
    }
    State::Claimed { to, at: Where::On(channel) }
}

/// Waiting, placed: assigned to the worker of `channel`, which keeps its
/// workstream from now on.
#[expect(clippy::too_many_arguments, reason = "a cell handler takes the fields it touches")]
fn assigned(
    to: ReplyTo,
    workstream: Box<[u8]>,
    channel: Id<Channel>,
    names: Names,
    env: &Env<Limits>,
    channels: &mut Slab<Channel>,
    facts: &mut Facts,
    out: &mut Queue<Request>,
) -> State {
    let entry = channels.get_mut(channel).expect("a worker chosen is in contact");
    channel::cache(entry, &env.limits, workstream);
    out.push(Request::Assign { channel: entry.token, run: names.run, attempt: names.attempt });
    out.push(Request::Placed { run: names.run, attempt: names.attempt });
    facts.push(Fact::Placed);
    State::Claimed { to, at: Where::On(channel) }
}

/// Waiting, listed: a worker hosts it already, so it is not assigned again.
#[expect(clippy::too_many_arguments, reason = "a cell handler takes the fields it touches")]
fn hosted(
    to: ReplyTo,
    workstream: Box<[u8]>,
    channel: Id<Channel>,
    names: Names,
    env: &Env<Limits>,
    channels: &mut Slab<Channel>,
    facts: &mut Facts,
    out: &mut Queue<Request>,
) -> State {
    let entry = channels.get_mut(channel).expect("a worker saying hello is in contact");
    channel::cache(entry, &env.limits, workstream);
    located(to, Where::On(channel), names, facts, out)
}

/// Listed, and not tracked: fenced off at once if its run is claimed by
/// another attempt, and otherwise a stray that waits to be adopted.
fn found(
    model: &mut Model,
    env: &Env<Limits>,
    channel: Id<Channel>,
    names: Names,
    answered: bool,
    out: &mut Queue<Request>,
) {
    let claimed = match model.runs.get(&names.run) {
        Some(run) => run.claim.is_some(),
        None => false,
    };
    let at = Where::On(channel);
    let state = if claimed {
        model.facts.push(Fact::Fenced);
        again(at, answered, names, &model.channels, out);
        State::Fenced { at }
    } else {
        model.facts.push(Fact::Stray);
        State::Stray { at, until: env.now.saturating_add(env.limits.grace) }
    };
    insert(model, names.run, names.attempt, state);
}

// What a state implies, in one place.

/// Tracks a new attempt in `state`. The entrance checked there is room.
fn insert(model: &mut Model, run: Token, attempt: Token, state: State) {
    let Ok(id) = model.attempts.insert(Attempt { run, token: attempt, state }) else {
        unreachable!("the entrance checks there is room for an attempt");
    };
    let named = model.names.insert((run, attempt), id);
    assert!(named == Ok(None), "an attempt tracked is named once, with room for every one");
    follow(model, id, UNTRACKED);
}

/// Derives what the new state of the attempt `id` implies, given what its
/// state `before` did: the slot it takes, its run's claim and how many of
/// its attempts a worker may host, its place in the queue, its alarm; and
/// retires it once it has closed.
fn follow(model: &mut Model, id: Id<Attempt>, before: Implied) {
    let entry = model.attempts.get(id).expect("an attempt lives until it is reclaimed");
    let after = implied(&entry.state);
    let run = entry.run;
    let attempt = entry.token;
    if before.on != after.on {
        if let Some(channel) = before.on {
            // Lost already, if it went adrift with its channel.
            if let Some(entry) = model.channels.get_mut(channel) {
                entry.hosts.remove(&id);
            }
            model.placing = true;
        }
        if let Some(channel) = after.on {
            let entry = model.channels.get_mut(channel).expect("an attempt is on a worker in contact");
            let room = entry.hosts.insert(id);
            assert!(room.is_ok(), "a worker hosts no more attempts than a hello may list or its slots take");
        }
    }
    if before.queued != after.queued {
        if let Some(serial) = before.queued {
            model.waiting.remove(&serial);
        }
        if let Some(serial) = after.queued {
            let room = model.waiting.insert(serial, id);
            assert!(room.is_ok(), "the queue has room for every attempt");
        }
        model.placing = true;
    }
    let mut entry = match model.runs.get(&run) {
        Some(entry) => *entry,
        None => Run { claim: None, held: 0 },
    };
    if before.held != after.held {
        entry.held = if after.held {
            entry.held.checked_add(1).expect("no more attempts held than tracked")
        } else {
            model.placing = true;
            entry.held.checked_sub(1).expect("an attempt held was counted")
        };
    }
    if after.claim {
        entry.claim = Some(id);
    } else if before.claim && entry.claim == Some(id) {
        entry.claim = None;
    }
    if entry.claim.is_none() && entry.held == 0 {
        model.runs.remove(&run);
    } else {
        let room = model.runs.insert(run, entry);
        assert!(room.is_ok(), "a run tracked has an attempt tracked");
    }
    match after.deadline {
        Some(at) => {
            let room = model.alarms.arm(id, at);
            assert!(room.is_ok(), "an alarm per attempt");
        }
        None => model.alarms.cancel(id),
    }
    if after.closed {
        model.names.remove(&(run, attempt));
        model.attempts.retire(id);
    }
}
