//! Tracked items: one item's lifecycle, from the moment it is taken in until
//! its step is done (engine-model.md, 4.2).
//!
//! A waiting item asks what is due. A run that is due is claimed first: the
//! record names the attempt, which only grows, and the run starts once the
//! record is written, never before. While it is in flight, inbox events are
//! relayed to it. Its answer is taken only for the attempt in flight: an
//! outcome is posted on the item, the record says it is being applied, and
//! only then is the answer acknowledged and the outcome applied; the record's
//! next update is the commit point (4.4). A run that parks leaves the item
//! waiting for its next wake. A run that fails is retried after a jittered
//! backoff as often as its failure class allows, and then the item is held
//! for a person; so is a run presumed lost. An engine action that is due is
//! made, and committed, as an outcome is (4.5). A person's stop holds the
//! item once its run has answered; a release makes it due again.
//!
//! The record is written at every step that must outlive the process, so the
//! hub can be rebuilt from the records alone (section 12): an item read
//! waiting or parked asks what is due; retrying, it backs off again; claimed,
//! it waits for a worker to say it still hosts the attempt, and past the
//! grace the run is presumed lost and retried with the next attempt;
//! applying, it applies the outcome again, and its keyed writes find what the
//! last process made; held, it waits for a person.
//!
//! An item's transition table:
//!
//! ```text
//! state       event                          next        emits
//! -           take, the working set full,
//!               taken in, or done            -           refused
//!             take, new                      Writing     taken, write: waiting (then ask)
//!             take, waiting or parked        Asking      taken, due
//!             take, retrying                 Backoff     taken
//!             take, claimed                  Running     taken (its grace runs)
//!             take, applying                 Applying    taken, apply
//!             take, held, or mangled         Held        taken
//!             stop, release                  -           refused: unknown
//!             answered                       -           stale
//!             running, inbox                 -           (the parent's to decide)
//! Writing     written                        as `next`   acknowledge an answer that waits;
//!                                                        due, apply, or left
//!             written: failed                Held        acknowledge an answer that waits
//! Claiming    written                        Running     start
//!             written, stopped               Writing     write: held (stopped)
//!             written: failed                Held
//!             stop                           Claiming    stopped
//! Asking      decided: nothing               Waiting     (until the earliest wake)
//!             decided: run                   Claiming    write: claimed, the next attempt
//!             decided: act, done             Acting      act
//!             decided: hold                  Writing     write: held (plan)
//!             inbox                          Asking      (the earliest wake kept)
//! Waiting     inbox: wakes now               Asking      due
//!             inbox: wakes later             Waiting     (the earlier)
//!             alarm                          Asking      due
//! Running     inbox                          Running     relay, unless stopped
//!             running                        Running     (its grace ends)
//!             stop                           Running     stopped, cancel (once)
//!             answered: ended                Recording   record
//!             answered: parked               Writing     keep, write: parked (then ask)
//!             answered: failed, lost         Writing     write: retrying (then backoff),
//!                                                        or held (failures)
//!             stopped, answered: not ended   Writing     keep, write: held (stopped)
//!             alarm: the grace is over       Writing     cancel, as lost
//! Recording   recorded                       Writing     write: applying (then apply)
//!             recorded: none                 Writing     write: held (writes)
//! Applying    applied: made                  Writing     write: waiting (then ask),
//!                                                        or held (plan)
//!             applied: stale                 Writing     write: waiting (then ask)
//!             applied: invalid               Writing     as failed
//!             stopped, applied: made, stale,
//!               invalid                      Writing     write: held (stopped)
//!             applied: accepting             Writing     write: held (acceptance)
//!             applied: failed                Writing     write: held (writes)
//! Acting      acted: made                    Writing     write: waiting (then ask),
//!                                                        or done (then leave)
//!             acted: stale                   Asking      due
//!             acted: accepting               Writing     write: held (acceptance)
//!             acted: failed                  Writing     write: held (writes)
//! Backoff     alarm                          Asking      due
//! Held        release                        Writing     released, write: waiting (then
//!                                                        ask), or applying (then apply)
//! any other   stop                           as it was   refused: idle
//!             release                        as it was   refused: unheld
//!             inbox                          as it was   (it asks what is due after)
//! any         running, not the attempt
//!               in flight                    as it was   cancel
//!             answered, not the attempt
//!               in flight                    as it was   stale
//! ```
//!
//! An answer of the attempt in flight is acknowledged once it is on the
//! forge: an outcome once the record says it is being applied, a park or a
//! failure once the record says so. A stopped run's answer still counts: an
//! outcome is applied (what landed is the truth), and then the item is held
//! rather than waiting. Every terminal event comes to the state that made
//! its request (one request in flight per item), so every other cell of a
//! terminal is unreachable by the contract; so is an alarm in a state that
//! arms none ([`follow`]).
//!
//! An item whose record did not decode counts its attempts from zero, and
//! from the highest attempt the fleet says of it while it is held, so its next
//! claim is past every attempt a worker may still hold.

use core::mem;

use temper_lib::{Duration, Env, Id, Queue, ReplyTo, Rng, Time, Token};

use crate::boundary::{
    Acted, Answer, Applied, Class, Due, Failures, Hold, Item, Lifecycle, Phase, Read, Refusal, Request, Then, Wrote,
};
use crate::facts::{Fact, Facts};
use crate::limits::Limits;
use crate::model::Model;

#[derive(Debug)]
pub(crate) struct Tracked {
    item: Item,
    /// The attempts made at its runs: the last claim's.
    attempts: u64,
    /// Its failures since its last run that did not fail, or its release.
    failures: Failures,
    state: State,
}

#[derive(Debug)]
enum State {
    /// Its record is being written. Once it is, the answer of the attempt
    /// `ack` is acknowledged if there is one, and the item goes on as `next`
    /// says.
    Writing { next: Next, ack: Option<u64> },
    /// Its claim of the attempt `attempts` is being written. The run the parent
    /// named `run` starts once it is, unless a person `stopped` it meanwhile.
    Claiming { run: Token, stopped: bool },
    /// It asks what is due. An inbox event that came meanwhile asked to wake
    /// it at `wake`, the earliest one.
    Asking { wake: Option<Time> },
    /// Nothing is due: it asks again at `until`, or when an inbox event
    /// wakes it.
    Waiting { until: Option<Time> },
    /// The attempt `attempts` is in flight. Read claimed after a restart, it
    /// waits for a worker's word until its `grace` is over. A person
    /// `stopped` it: it is cancelled.
    Running { grace: Option<Time>, stopped: bool },
    /// The attempt's outcome is being posted on the item.
    Recording { stopped: bool },
    /// The outcome posted as the comment `outcome` is being applied.
    Applying { outcome: u64, stopped: bool },
    /// An engine action's writes are being made; `done` if they finish the
    /// step.
    Acting { done: bool },
    /// It is retried once `until` has passed.
    Backoff { until: Time },
    /// Held for a person.
    Held { why: Hold },
    /// Terminal: its step is done.
    Closed,
}

/// What an item does once its record is written.
#[derive(Debug)]
enum Next {
    /// It asks what is due.
    Ask,
    /// It applies the outcome posted as the comment `outcome`.
    Apply { outcome: u64, stopped: bool },
    /// It is retried after a backoff, for a failure in `class`.
    Backoff { class: Class },
    /// It is held for a person.
    Held { why: Hold },
    /// It leaves: its step is done.
    Leave,
}

// Entry points, one per event: look the item up, take its state out, run the
// cell's handler, follow from the item's new state.

pub(crate) fn take(
    model: &mut Model,
    env: &Env<Limits>,
    reply_to: ReplyTo,
    item: Item,
    read: Read,
    out: &mut Queue<Request>,
) {
    let Model { tracked, names, rng, facts, .. } = model;
    if names.contains_key(&item) {
        out.push(Request::Refused { to: reply_to, refusal: Refusal::Taken });
        return;
    }
    let (attempts, failures) = match read {
        Read::New | Read::Mangled => (0, Failures::NONE),
        Read::Record(Lifecycle { phase: Phase::Done, .. }) => {
            out.push(Request::Refused { to: reply_to, refusal: Refusal::Done });
            return;
        }
        Read::Record(lifecycle) => (lifecycle.attempts, lifecycle.failures),
    };
    let entry = Tracked { item, attempts, failures, state: State::Closed };
    let Ok(id) = tracked.insert(entry) else {
        out.push(Request::Refused { to: reply_to, refusal: Refusal::Full });
        return;
    };
    let named = names.insert(item, id).expect("a name for every item");
    assert!(named.is_none(), "checked the item is not taken in above");
    facts.push(Fact::Taken { item });
    out.push(Request::Taken { to: reply_to });
    let entry = tracked.get_mut(id).expect("just taken in");
    entry.state = match read {
        Read::New => writing(entry, id, Phase::Waiting, Next::Ask, None, out),
        Read::Mangled => held(facts, item, Hold::Record),
        Read::Record(Lifecycle { phase, .. }) => match phase {
            Phase::Waiting | Phase::Parked => ask(entry, id, out),
            Phase::Retrying(class) => State::Backoff { until: backoff(entry.failures, class, env, rng) },
            Phase::Claimed => State::Running { grace: Some(env.now.saturating_add(env.limits.grace)), stopped: false },
            Phase::Applying { outcome } => apply(entry, id, outcome, false, out),
            Phase::Held(why) => held(facts, item, why),
            Phase::Done => unreachable!("refused above: a done item is not live"),
        },
    };
    follow(model, id, out);
}

pub(crate) fn stop(model: &mut Model, reply_to: ReplyTo, item: Item, out: &mut Queue<Request>) {
    let Some(id) = named(model, item) else {
        out.push(Request::Refused { to: reply_to, refusal: Refusal::Unknown });
        return;
    };
    let entry = model.tracked.get_mut(id).expect("a named item is tracked");
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Claiming { run, stopped: _ } => {
            out.push(Request::Stopped { to: reply_to });
            State::Claiming { run, stopped: true }
        }
        State::Running { grace, stopped } => cancel(entry, reply_to, grace, stopped, out),
        state @ (State::Writing { .. }
        | State::Asking { .. }
        | State::Waiting { .. }
        | State::Recording { .. }
        | State::Applying { .. }
        | State::Acting { .. }
        | State::Backoff { .. }
        | State::Held { .. }
        | State::Closed) => {
            out.push(Request::Refused { to: reply_to, refusal: Refusal::Idle });
            state
        }
    };
    follow(model, id, out);
}

pub(crate) fn release(model: &mut Model, reply_to: ReplyTo, item: Item, out: &mut Queue<Request>) {
    let Some(id) = named(model, item) else {
        out.push(Request::Refused { to: reply_to, refusal: Refusal::Unknown });
        return;
    };
    let Model { tracked, facts, .. } = model;
    let entry = tracked.get_mut(id).expect("a named item is tracked");
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Held { why } => released(entry, id, facts, reply_to, why, out),
        state @ (State::Writing { .. }
        | State::Claiming { .. }
        | State::Asking { .. }
        | State::Waiting { .. }
        | State::Running { .. }
        | State::Recording { .. }
        | State::Applying { .. }
        | State::Acting { .. }
        | State::Backoff { .. }
        | State::Closed) => {
            out.push(Request::Refused { to: reply_to, refusal: Refusal::Unheld });
            state
        }
    };
    follow(model, id, out);
}

pub(crate) fn inbox(
    model: &mut Model,
    env: &Env<Limits>,
    item: Item,
    event: Token,
    wake: Option<Time>,
    out: &mut Queue<Request>,
) {
    let Some(id) = named(model, item) else {
        return;
    };
    let entry = model.tracked.get_mut(id).expect("a named item is tracked");
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Asking { wake: earlier } => State::Asking { wake: earliest(earlier, wake) },
        State::Waiting { until } => woken(entry, id, env, until, wake, out),
        State::Running { grace, stopped } => {
            if !stopped {
                out.push(Request::Relay { item, attempt: entry.attempts, event });
            }
            State::Running { grace, stopped }
        }
        // It asks what is due once it is through, or is held until a person
        // releases it; a claimed run's brief is rendered as it starts.
        state @ (State::Writing { .. }
        | State::Claiming { .. }
        | State::Recording { .. }
        | State::Applying { .. }
        | State::Acting { .. }
        | State::Backoff { .. }
        | State::Held { .. }
        | State::Closed) => state,
    };
    follow(model, id, out);
}

pub(crate) fn running(model: &mut Model, item: Item, attempt: u64, out: &mut Queue<Request>) {
    // A run of an item not taken in is the parent's to decide: the item may
    // be taken in later, and its run then adopted.
    let Some(id) = named(model, item) else {
        return;
    };
    let Model { tracked, facts, .. } = model;
    let entry = tracked.get_mut(id).expect("a named item is tracked");
    raise(entry, attempt);
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Running { grace: _, stopped } if attempt == entry.attempts => {
            facts.push(Fact::Running { item, attempt });
            State::Running { grace: None, stopped }
        }
        state @ (State::Writing { .. }
        | State::Claiming { .. }
        | State::Asking { .. }
        | State::Waiting { .. }
        | State::Running { .. }
        | State::Recording { .. }
        | State::Applying { .. }
        | State::Acting { .. }
        | State::Backoff { .. }
        | State::Held { .. }
        | State::Closed) => {
            out.push(Request::Cancel { item, attempt });
            state
        }
    };
    follow(model, id, out);
}

pub(crate) fn answered(
    model: &mut Model,
    env: &Env<Limits>,
    item: Item,
    attempt: u64,
    answer: Answer,
    out: &mut Queue<Request>,
) {
    let Some(id) = named(model, item) else {
        out.push(Request::Stale { item, attempt });
        return;
    };
    let Model { tracked, facts, .. } = model;
    let entry = tracked.get_mut(id).expect("a named item is tracked");
    raise(entry, attempt);
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Running { grace: _, stopped } if attempt == entry.attempts => match answer {
            Answer::Ended { outcome } => ended(entry, id, facts, outcome, stopped, out),
            Answer::Parked { snapshot } => parked(entry, id, facts, snapshot, stopped, out),
            Answer::Failed(class) => failed(entry, id, env, facts, class, Some(attempt), stopped, out),
            Answer::Lost => failed(entry, id, env, facts, Class::Lost, None, stopped, out),
        },
        state @ (State::Writing { .. }
        | State::Claiming { .. }
        | State::Asking { .. }
        | State::Waiting { .. }
        | State::Running { .. }
        | State::Recording { .. }
        | State::Applying { .. }
        | State::Acting { .. }
        | State::Backoff { .. }
        | State::Held { .. }
        | State::Closed) => {
            out.push(Request::Stale { item, attempt });
            state
        }
    };
    follow(model, id, out);
}

pub(crate) fn decided(model: &mut Model, owner: Token, due: Due, out: &mut Queue<Request>) {
    let id = Id::from_token(owner);
    let Model { tracked, facts, .. } = model;
    let entry = tracked.get_mut(id).expect("a token travelling up is never stale");
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Asking { wake } => match due {
            Due::Nothing { until } => nothing(wake, until),
            Due::Run { run } => claim(entry, id, facts, run, out),
            Due::Act { action } => act(entry, id, action, false, out),
            Due::Done { action } => act(entry, id, action, true, out),
            Due::Hold { why } => hold(entry, id, Hold::Plan(why), None, out),
        },
        State::Writing { .. }
        | State::Claiming { .. }
        | State::Waiting { .. }
        | State::Running { .. }
        | State::Recording { .. }
        | State::Applying { .. }
        | State::Acting { .. }
        | State::Backoff { .. }
        | State::Held { .. }
        | State::Closed => unreachable!("only an item asking has a decision in flight"),
    };
    follow(model, id, out);
}

pub(crate) fn written(model: &mut Model, env: &Env<Limits>, owner: Token, wrote: Wrote, out: &mut Queue<Request>) {
    let id = Id::from_token(owner);
    let Model { tracked, rng, facts, .. } = model;
    let entry = tracked.get_mut(id).expect("a token travelling up is never stale");
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Writing { next, ack } => {
            if let Some(attempt) = ack {
                out.push(Request::Acknowledge { item: entry.item, attempt });
            }
            match wrote {
                Wrote::Done => proceed(entry, id, env, rng, facts, next, out),
                Wrote::Failed => held(facts, entry.item, Hold::Record),
            }
        }
        State::Claiming { run, stopped } => match wrote {
            Wrote::Done => claimed(entry, id, run, stopped, out),
            Wrote::Failed => held(facts, entry.item, Hold::Record),
        },
        State::Asking { .. }
        | State::Waiting { .. }
        | State::Running { .. }
        | State::Recording { .. }
        | State::Applying { .. }
        | State::Acting { .. }
        | State::Backoff { .. }
        | State::Held { .. }
        | State::Closed => unreachable!("only an item writing its record has a write in flight"),
    };
    follow(model, id, out);
}

pub(crate) fn recorded(model: &mut Model, owner: Token, comment: Option<u64>, out: &mut Queue<Request>) {
    let id = Id::from_token(owner);
    let entry = model.tracked.get_mut(id).expect("a token travelling up is never stale");
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Recording { stopped } => {
            let ack = Some(entry.attempts);
            match comment {
                Some(outcome) => {
                    let next = Next::Apply { outcome, stopped };
                    writing(entry, id, Phase::Applying { outcome }, next, ack, out)
                }
                None => hold(entry, id, Hold::Writes, ack, out),
            }
        }
        State::Writing { .. }
        | State::Claiming { .. }
        | State::Asking { .. }
        | State::Waiting { .. }
        | State::Running { .. }
        | State::Applying { .. }
        | State::Acting { .. }
        | State::Backoff { .. }
        | State::Held { .. }
        | State::Closed => unreachable!("only an item recording an outcome has a post in flight"),
    };
    follow(model, id, out);
}

pub(crate) fn applied(model: &mut Model, env: &Env<Limits>, owner: Token, applied: Applied, out: &mut Queue<Request>) {
    let id = Id::from_token(owner);
    let Model { tracked, facts, .. } = model;
    let entry = tracked.get_mut(id).expect("a token travelling up is never stale");
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Applying { outcome, stopped } => match applied {
            Applied::Made(then) => made(entry, id, facts, then, stopped, out),
            Applied::Stale => stale(entry, id, facts, stopped, out),
            Applied::Invalid => failed(entry, id, env, facts, Class::Invalid, None, stopped, out),
            Applied::Accepting => hold(entry, id, Hold::Acceptance { outcome: Some(outcome) }, None, out),
            Applied::Failed => hold(entry, id, Hold::Writes, None, out),
        },
        State::Writing { .. }
        | State::Claiming { .. }
        | State::Asking { .. }
        | State::Waiting { .. }
        | State::Running { .. }
        | State::Recording { .. }
        | State::Acting { .. }
        | State::Backoff { .. }
        | State::Held { .. }
        | State::Closed => unreachable!("only an item applying an outcome has an application in flight"),
    };
    follow(model, id, out);
}

pub(crate) fn acted(model: &mut Model, owner: Token, acted: Acted, out: &mut Queue<Request>) {
    let id = Id::from_token(owner);
    let Model { tracked, facts, .. } = model;
    let entry = tracked.get_mut(id).expect("a token travelling up is never stale");
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Acting { done } => match acted {
            Acted::Made => {
                facts.push(Fact::Applied { item: entry.item });
                if done {
                    writing(entry, id, Phase::Done, Next::Leave, None, out)
                } else {
                    writing(entry, id, Phase::Waiting, Next::Ask, None, out)
                }
            }
            Acted::Stale => {
                facts.push(Fact::Stale { item: entry.item });
                ask(entry, id, out)
            }
            Acted::Accepting => hold(entry, id, Hold::Acceptance { outcome: None }, None, out),
            Acted::Failed => hold(entry, id, Hold::Writes, None, out),
        },
        State::Writing { .. }
        | State::Claiming { .. }
        | State::Asking { .. }
        | State::Waiting { .. }
        | State::Running { .. }
        | State::Recording { .. }
        | State::Applying { .. }
        | State::Backoff { .. }
        | State::Held { .. }
        | State::Closed => unreachable!("only an item acting has an action in flight"),
    };
    follow(model, id, out);
}

pub(crate) fn alarm(model: &mut Model, env: &Env<Limits>, id: Id<Tracked>, out: &mut Queue<Request>) {
    let Model { tracked, facts, .. } = model;
    let entry = tracked.get_mut(id).expect("an item's alarm is cancelled as it closes");
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Waiting { .. } | State::Backoff { .. } => ask(entry, id, out),
        State::Running { grace: Some(_), stopped } => lapsed(entry, id, env, facts, stopped, out),
        State::Running { grace: None, .. } => unreachable!("only a grace arms a running item's alarm"),
        State::Writing { .. }
        | State::Claiming { .. }
        | State::Asking { .. }
        | State::Recording { .. }
        | State::Applying { .. }
        | State::Acting { .. }
        | State::Held { .. }
        | State::Closed => unreachable!("an alarm is armed only while waiting, backing off or in a grace"),
    };
    follow(model, id, out);
}

fn named(model: &Model, item: Item) -> Option<Id<Tracked>> {
    model.names.get(&item).copied()
}

/// Derives what the item's state implies, after every transition: its alarm,
/// and, once it is Closed, its retirement.
fn follow(model: &mut Model, id: Id<Tracked>, out: &mut Queue<Request>) {
    let Model { tracked, names, alarms, facts, .. } = model;
    let entry = tracked.get(id).expect("an item lives until it is retired");
    let (alarm, closed) = match &entry.state {
        State::Waiting { until } => (*until, false),
        State::Backoff { until } => (Some(*until), false),
        State::Running { grace, .. } => (*grace, false),
        State::Closed => (None, true),
        State::Writing { .. }
        | State::Claiming { .. }
        | State::Asking { .. }
        | State::Recording { .. }
        | State::Applying { .. }
        | State::Acting { .. }
        | State::Held { .. } => (None, false),
    };
    match alarm {
        Some(at) => alarms.arm(id, at).expect("room for an alarm per item"),
        None => alarms.cancel(id),
    }
    if closed {
        let item = entry.item;
        names.remove(&item);
        tracked.retire(id);
        facts.push(Fact::Done { item });
        out.push(Request::Left { item });
    }
}

// Cell handlers and what they share: each takes the source state's data by
// value and returns the target state.

/// Asks what is due.
fn ask(entry: &Tracked, id: Id<Tracked>, out: &mut Queue<Request>) -> State {
    out.push(Request::Due { owner: id.token(), item: entry.item });
    State::Asking { wake: None }
}

/// Writes the record with `phase`; the item goes on as `next` says once it
/// is written, having acknowledged the answer of the attempt `ack`.
fn writing(
    entry: &Tracked,
    id: Id<Tracked>,
    phase: Phase,
    next: Next,
    ack: Option<u64>,
    out: &mut Queue<Request>,
) -> State {
    let lifecycle = Lifecycle { phase, attempts: entry.attempts, failures: entry.failures };
    out.push(Request::Write { owner: id.token(), item: entry.item, lifecycle });
    State::Writing { next, ack }
}

/// Writes the record held for `why`, and holds the item once it is written.
fn hold(entry: &Tracked, id: Id<Tracked>, why: Hold, ack: Option<u64>, out: &mut Queue<Request>) -> State {
    writing(entry, id, Phase::Held(why), Next::Held { why }, ack, out)
}

/// The item is held, as its record says, or as it cannot.
fn held(facts: &mut Facts, item: Item, why: Hold) -> State {
    facts.push(Fact::Held { item, why });
    State::Held { why }
}

/// Applies the outcome posted as the comment `outcome`.
fn apply(entry: &Tracked, id: Id<Tracked>, outcome: u64, stopped: bool, out: &mut Queue<Request>) -> State {
    out.push(Request::Apply { owner: id.token(), item: entry.item, attempt: entry.attempts, outcome });
    State::Applying { outcome, stopped }
}

/// Writing, written: acknowledged already; the item goes on.
fn proceed(
    entry: &Tracked,
    id: Id<Tracked>,
    env: &Env<Limits>,
    rng: &mut Rng,
    facts: &mut Facts,
    next: Next,
    out: &mut Queue<Request>,
) -> State {
    match next {
        Next::Ask => ask(entry, id, out),
        Next::Apply { outcome, stopped } => apply(entry, id, outcome, stopped, out),
        Next::Backoff { class } => State::Backoff { until: backoff(entry.failures, class, env, rng) },
        Next::Held { why } => held(facts, entry.item, why),
        Next::Leave => State::Closed,
    }
}

/// Claiming, written: the claim is on the forge, so the run starts, unless a
/// person stopped it meanwhile.
fn claimed(entry: &Tracked, id: Id<Tracked>, run: Token, stopped: bool, out: &mut Queue<Request>) -> State {
    if stopped {
        return hold(entry, id, Hold::Stopped, None, out);
    }
    out.push(Request::Start { item: entry.item, attempt: entry.attempts, run });
    State::Running { grace: None, stopped: false }
}

/// Asking, decided: nothing is due. It waits until the earliest of the
/// plan's time and a wake that came meanwhile; one that is due already fires
/// in this iteration.
fn nothing(wake: Option<Time>, until: Option<Time>) -> State {
    State::Waiting { until: earliest(wake, until) }
}

/// Asking, decided: a run is due. It is claimed with the next attempt.
fn claim(entry: &mut Tracked, id: Id<Tracked>, facts: &mut Facts, run: Token, out: &mut Queue<Request>) -> State {
    entry.attempts = entry.attempts.saturating_add(1);
    facts.push(Fact::Claimed { item: entry.item, attempt: entry.attempts });
    let lifecycle = Lifecycle { phase: Phase::Claimed, attempts: entry.attempts, failures: entry.failures };
    out.push(Request::Write { owner: id.token(), item: entry.item, lifecycle });
    State::Claiming { run, stopped: false }
}

/// Asking, decided: an engine action is due, or the writes that finish the
/// step.
fn act(entry: &Tracked, id: Id<Tracked>, action: Token, done: bool, out: &mut Queue<Request>) -> State {
    out.push(Request::Act { owner: id.token(), item: entry.item, action });
    State::Acting { done }
}

/// Waiting, inbox: a wake now asks what is due; a later one is kept, the
/// earliest.
fn woken(
    entry: &Tracked,
    id: Id<Tracked>,
    env: &Env<Limits>,
    until: Option<Time>,
    wake: Option<Time>,
    out: &mut Queue<Request>,
) -> State {
    match wake {
        Some(at) if at <= env.now => ask(entry, id, out),
        Some(_) | None => State::Waiting { until: earliest(until, wake) },
    }
}

/// Running, stop: the run is cancelled, once.
fn cancel(entry: &Tracked, reply_to: ReplyTo, grace: Option<Time>, stopped: bool, out: &mut Queue<Request>) -> State {
    out.push(Request::Stopped { to: reply_to });
    if !stopped {
        out.push(Request::Cancel { item: entry.item, attempt: entry.attempts });
    }
    State::Running { grace, stopped: true }
}

/// Running, answered: the run ended with an outcome, which is posted first.
fn ended(
    entry: &Tracked,
    id: Id<Tracked>,
    facts: &mut Facts,
    outcome: Token,
    stopped: bool,
    out: &mut Queue<Request>,
) -> State {
    let (item, attempt) = (entry.item, entry.attempts);
    facts.push(Fact::Ended { item, attempt });
    out.push(Request::Record { owner: id.token(), item, attempt, outcome });
    State::Recording { stopped }
}

/// Running, answered: the run parked. Its snapshot is kept, and the item
/// waits for its next wake, or is held if a person stopped it.
fn parked(
    entry: &mut Tracked,
    id: Id<Tracked>,
    facts: &mut Facts,
    snapshot: Option<Token>,
    stopped: bool,
    out: &mut Queue<Request>,
) -> State {
    let (item, attempt) = (entry.item, entry.attempts);
    facts.push(Fact::Parked { item, attempt });
    if let Some(snapshot) = snapshot {
        out.push(Request::Keep { item, attempt, snapshot });
    }
    entry.failures = Failures::NONE;
    if stopped {
        return hold(entry, id, Hold::Stopped, Some(attempt), out);
    }
    writing(entry, id, Phase::Parked, Next::Ask, Some(attempt), out)
}

/// Running, answered: the run failed in `class` (or was lost, with nothing
/// to acknowledge: `ack` is `None`); so did an outcome that is invalid. It is
/// retried after a backoff, as often as the class allows, and then held; a
/// stopped one is held at once.
#[expect(clippy::too_many_arguments, reason = "a cell handler takes what its cell touches")]
fn failed(
    entry: &mut Tracked,
    id: Id<Tracked>,
    env: &Env<Limits>,
    facts: &mut Facts,
    class: Class,
    ack: Option<u64>,
    stopped: bool,
    out: &mut Queue<Request>,
) -> State {
    facts.push(Fact::Failed { item: entry.item, attempt: entry.attempts, class });
    if stopped {
        return hold(entry, id, Hold::Stopped, ack, out);
    }
    entry.failures = entry.failures.and(class);
    if entry.failures.of(class) > env.limits.retries.of(class).retries {
        return hold(entry, id, Hold::Failures(class), ack, out);
    }
    writing(entry, id, Phase::Retrying(class), Next::Backoff { class }, ack, out)
}

/// Running, alarm: no worker said it still hosts the attempt read claimed,
/// within the grace. The run is presumed lost; the attempt is cancelled, so
/// that a worker that comes back later drops it.
fn lapsed(
    entry: &mut Tracked,
    id: Id<Tracked>,
    env: &Env<Limits>,
    facts: &mut Facts,
    stopped: bool,
    out: &mut Queue<Request>,
) -> State {
    out.push(Request::Cancel { item: entry.item, attempt: entry.attempts });
    failed(entry, id, env, facts, Class::Lost, None, stopped, out)
}

/// Applying, applied: the outcome's writes are made. The record says what
/// the item does next, the commit point.
fn made(
    entry: &mut Tracked,
    id: Id<Tracked>,
    facts: &mut Facts,
    then: Then,
    stopped: bool,
    out: &mut Queue<Request>,
) -> State {
    facts.push(Fact::Applied { item: entry.item });
    entry.failures = Failures::NONE;
    if stopped {
        return hold(entry, id, Hold::Stopped, None, out);
    }
    match then {
        Then::Wait => writing(entry, id, Phase::Waiting, Next::Ask, None, out),
        Then::Hold(why) => hold(entry, id, Hold::Plan(why), None, out),
    }
}

/// Applying, applied: the outcome is stale, and nothing of it is made. The
/// run did not fail: the item moved on.
fn stale(entry: &mut Tracked, id: Id<Tracked>, facts: &mut Facts, stopped: bool, out: &mut Queue<Request>) -> State {
    facts.push(Fact::Stale { item: entry.item });
    entry.failures = Failures::NONE;
    if stopped {
        return hold(entry, id, Hold::Stopped, None, out);
    }
    writing(entry, id, Phase::Waiting, Next::Ask, None, out)
}

/// Held, release: an outcome waiting for a person's acceptance is applied
/// again; any other item is due again, with its failures forgiven.
fn released(
    entry: &mut Tracked,
    id: Id<Tracked>,
    facts: &mut Facts,
    reply_to: ReplyTo,
    why: Hold,
    out: &mut Queue<Request>,
) -> State {
    out.push(Request::Released { to: reply_to });
    facts.push(Fact::Released { item: entry.item });
    match why {
        Hold::Acceptance { outcome: Some(outcome) } => {
            let next = Next::Apply { outcome, stopped: false };
            writing(entry, id, Phase::Applying { outcome }, next, None, out)
        }
        Hold::Acceptance { outcome: None }
        | Hold::Plan(_)
        | Hold::Failures(_)
        | Hold::Stopped
        | Hold::Writes
        | Hold::Record => {
            entry.failures = Failures::NONE;
            writing(entry, id, Phase::Waiting, Next::Ask, None, out)
        }
    }
}

/// An item whose record did not decode learns its attempts from what the
/// fleet says of them, so its next claim is past every one a worker holds.
fn raise(entry: &mut Tracked, attempt: u64) {
    match entry.state {
        State::Held { why: Hold::Record } => entry.attempts = entry.attempts.max(attempt),
        State::Held { .. }
        | State::Writing { .. }
        | State::Claiming { .. }
        | State::Asking { .. }
        | State::Waiting { .. }
        | State::Running { .. }
        | State::Recording { .. }
        | State::Applying { .. }
        | State::Acting { .. }
        | State::Backoff { .. }
        | State::Closed => {}
    }
}

/// The earlier of two times, either of which may be missing.
fn earliest(one: Option<Time>, other: Option<Time>) -> Option<Time> {
    match one {
        Some(one) => match other {
            Some(other) => Some(one.min(other)),
            None => Some(one),
        },
        None => other,
    }
}

/// When to retry an item whose failures are `failures`, the last in `class`:
/// exponential in the failures of the class and capped, with equal jitter,
/// half fixed and half random.
fn backoff(failures: Failures, class: Class, env: &Env<Limits>, rng: &mut Rng) -> Time {
    let retry = env.limits.retries.of(class);
    let doublings = failures.of(class).saturating_sub(1);
    let factor = 1_u64.checked_shl(doublings).unwrap_or(u64::MAX);
    let ceiling = retry.base.saturating_mul(factor).min(retry.max);
    let half = ceiling.as_nanos() / 2;
    let jittered = Duration::from_nanos(half.saturating_add(rng.below(half.saturating_add(1))));
    env.now.saturating_add(jittered)
}
