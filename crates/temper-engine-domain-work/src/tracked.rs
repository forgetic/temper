//! Tracked items: one item's lifecycle, from the moment it is taken in until
//! its step is done (engine-domain.md, 4.2).
//!
//! A waiting item asks what is due. A run that is due is claimed first: the
//! record names the attempt, which only grows, and the run starts once the
//! record is written, never before. While it is in flight, inbox events are
//! relayed to it, and those the fleet could not deliver yet are kept and
//! relayed again once it is placed. Its answer comes once, for the attempt in
//! flight: an outcome is posted on the item, the record says it is being
//! applied, and only then is the answer acknowledged and the outcome applied;
//! the record's next update is the commit point (4.4). A run that parks
//! leaves the item waiting for its next wake. A run that fails is retried
//! after a jittered backoff as often as its failure class allows, and then
//! the item is held for a person; so is a run presumed lost. A run refused
//! before anything ran is no failure: the item claims again after a backoff.
//! An engine action that is due is made, and committed, as an outcome is
//! (4.5). A person's stop holds the item once its run has answered; a release
//! makes it due again, or applies again the outcome its hold keeps.
//!
//! The record is written at every step that must outlive the process, so the
//! hub can be rebuilt from the records alone (section 12): an item read
//! waiting or parked asks what is due; retrying, it backs off again; claimed,
//! its run is adopted, and the fleet says whether a worker still hosts it or
//! it is lost; applying, it applies the outcome again, and its keyed writes
//! find what the last process made; held, it waits for a person.
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
//!             take, claimed                  Running     taken, adopt
//!             take, applying                 Applying    taken, apply
//!             take, held, or mangled         Held        taken
//!             stop, release                  -           refused: unknown
//!             answered                       -           stale
//!             placed, undelivered, listed,
//!               inbox                        -           (the parent's to decide)
//! Writing     written                        as `next`   acknowledge an answer that waits;
//!                                                        due, apply, or left
//!             written: failed                Held        (keeping the answer that waits)
//!             answered: the one it waits
//!               to acknowledge               Writing     (its acknowledgement follows)
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
//!             undelivered                    Running     (kept, while there is room)
//!             placed                         Running     relay what was kept
//!             stop                           Running     stopped, cancel (once)
//!             answered: ended                Recording   record
//!             answered: parked               Writing     keep, write: parked (then ask)
//!             answered: failed, lost         Writing     write: retrying (then backoff),
//!                                                        or held (failures)
//!             answered: refused              Writing     write: waiting (then a pause)
//!             stopped, answered: not ended   Writing     keep, write: held (stopped)
//! Recording   recorded                       Writing     write: applying (then apply)
//!             recorded: none                 Writing     write: held (writes)
//!             answered: the one recorded     Recording   (its acknowledgement follows)
//! Applying    applied: made                  Writing     write: waiting (then ask),
//!                                                        or held (plan)
//!             applied: stale                 Writing     write: waiting (then ask)
//!             applied: invalid               Writing     as failed
//!             stopped, applied: made, stale,
//!               invalid                      Writing     write: held (stopped)
//!             applied: accepting             Writing     write: held (acceptance, with it)
//!             applied: failed                Writing     write: held (writes, with it)
//! Acting      acted: made                    Writing     write: waiting (then ask),
//!                                                        or done (then leave)
//!             acted: stale                   Asking      due
//!             acted: accepting               Writing     write: held (acceptance)
//!             acted: failed                  Writing     write: held (writes)
//! Backoff     alarm                          Asking      due
//! Held        release                        Writing     released, write: applying (then
//!                                                        apply) the outcome it keeps,
//!                                                        or waiting (then ask); then
//!                                                        acknowledge an answer it keeps
//!             answered: the one it keeps     Held        (its acknowledgement follows)
//!             listed                         Held        (a mangled record's attempts)
//! any other   stop                           as it was   refused: idle
//!             release                        as it was   refused: unheld
//!             inbox                          as it was   (it asks what is due after)
//!             placed, undelivered, listed    as it was
//!             answered                       as it was   stale
//! ```
//!
//! An answer of the attempt in flight is acknowledged once it is on the
//! forge: an outcome once the record says it is being applied, a park or a
//! failure once the record says so; until then a copy of it is dropped
//! without a word, never called stale, so its worker keeps it. If that
//! record cannot be written, the item is held keeping the answer, which the
//! write that releases it acknowledges. A stopped
//! run's answer still counts: an outcome is applied (what landed is the
//! truth), and then the item is held rather than waiting. A hold keeps the
//! outcome that is not wholly applied (one waiting for a person's
//! acceptance, one whose writes failed, one whose record could not be
//! written), and a release applies it again. Every terminal event comes to
//! the state that made its request (one request in flight per item), so
//! every other cell of a terminal is unreachable by the contract; so is an
//! alarm in a state that arms none ([`follow`]). The fleet runs the races on
//! a run (its grace, a cancel crossing an answer): the hub arms no timer on
//! one, and cancels it once.
//!
//! An item whose record did not decode counts its attempts from the highest
//! its outcomes on the forge name, and from the highest a worker lists of it
//! while it is held, so its next claim is past every attempt whose outcome is
//! posted or that a worker may still hold.

use core::mem;

use temper_lib::{Duration, Env, Id, Queue, ReplyTo, Rng, Time, Token};

use crate::boundary::{
    Acted, Answer, Applied, Class, Due, Failures, Hold, Item, Lifecycle, Phase, Read, Refusal, Request, Then, Wrote,
};
use crate::domain::Domain;
use crate::facts::{Fact, Facts};
use crate::limits::Limits;

#[derive(Debug)]
pub(crate) struct Tracked {
    item: Item,
    /// The attempts made at its runs: the last claim's.
    attempts: u64,
    /// Its failures since its last run that did not fail, or its release.
    failures: Failures,
    /// The runs refused in a row, before anything ran: what its next claim
    /// waits on.
    refusals: u32,
    /// Inbound events the fleet could not deliver yet to the run in flight,
    /// to relay again once it is placed.
    undelivered: Queue<Token>,
    state: State,
}

#[derive(Debug)]
enum State {
    /// Its record is being written. Once it is, it goes on as `next` says;
    /// what it leaves `behind` is the answer it acknowledges then, and the
    /// outcome it holds if the write fails.
    Writing { next: Next, behind: Behind },
    /// Its claim of the attempt `attempts` is being written. The run the parent
    /// named `run` starts once it is, unless a person `stopped` it meanwhile.
    Claiming { run: Token, stopped: bool },
    /// It asks what is due. An inbox event that came meanwhile asked to wake
    /// it at `wake`, the earliest one.
    Asking { wake: Option<Time> },
    /// Nothing is due: it asks again at `until`, or when an inbox event
    /// wakes it.
    Waiting { until: Option<Time> },
    /// The attempt `attempts` is in flight, started or adopted, until its
    /// answer. A person `stopped` it: it is cancelled.
    Running { stopped: bool },
    /// The attempt's outcome is being posted on the item.
    Recording { stopped: bool },
    /// The outcome posted as the comment `outcome` is being applied.
    Applying { outcome: u64, stopped: bool },
    /// An engine action's writes are being made; `done` if they finish the
    /// step.
    Acting { done: bool },
    /// It claims again, or asks what is due, once `until` has passed.
    Backoff { until: Time },
    /// Held for a person, keeping `behind` the outcome not wholly applied
    /// yet, and the answer it has not acknowledged, whose record could not be
    /// written: the write that releases it acknowledges it.
    Held { why: Hold, behind: Behind },
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
    /// It pauses after a run refused before anything ran, then asks again.
    Pause,
    /// It is held for a person, keeping `outcome`.
    Held { why: Hold, outcome: Option<u64> },
    /// It leaves: its step is done.
    Leave,
}

/// What a record write leaves behind: the answer of the attempt `ack`, which
/// it acknowledges once it lands, and the outcome posted as the comment
/// `outcome` that it commits or leads to, which the item holds if the write
/// fails.
#[derive(Clone, Copy, Debug)]
struct Behind {
    ack: Option<u64>,
    outcome: Option<u64>,
}

impl Behind {
    const NOTHING: Behind = Behind { ack: None, outcome: None };

    const fn ack(attempt: u64) -> Behind {
        Behind { ack: Some(attempt), outcome: None }
    }

    const fn outcome(outcome: u64) -> Behind {
        Behind { ack: None, outcome: Some(outcome) }
    }
}

// Entry points, one per event: look the item up, take its state out, run the
// cell's handler, follow from the item's new state.

pub(crate) fn take(
    domain: &mut Domain,
    env: &Env<Limits>,
    reply_to: ReplyTo,
    item: Item,
    read: Read,
    out: &mut Queue<Request>,
) {
    let Domain { tracked, names, rng, facts, .. } = domain;
    if names.contains_key(&item) {
        out.push(Request::Refused { to: reply_to, refusal: Refusal::Taken });
        return;
    }
    let (attempts, failures) = match read {
        Read::New => (0, Failures::NONE),
        Read::Mangled { attempts } => (attempts, Failures::NONE),
        Read::Record(Lifecycle { phase: Phase::Done, .. }) => {
            out.push(Request::Refused { to: reply_to, refusal: Refusal::Done });
            return;
        }
        Read::Record(lifecycle) => (lifecycle.attempts, lifecycle.failures),
    };
    if tracked.is_full() {
        out.push(Request::Refused { to: reply_to, refusal: Refusal::Full });
        return;
    }
    let undelivered = Queue::with_capacity(env.limits.undelivered);
    let entry = Tracked { item, attempts, failures, refusals: 0, undelivered, state: State::Closed };
    let id = tracked.insert(entry).expect("checked for room above");
    let named = names.insert(item, id).expect("a name for every item");
    assert!(named.is_none(), "checked the item is not taken in above");
    facts.push(Fact::Taken { item });
    out.push(Request::Taken { to: reply_to });
    let entry = tracked.get_mut(id).expect("just taken in");
    entry.state = match read {
        Read::New => writing(entry, id, Phase::Waiting, Next::Ask, Behind::NOTHING, out),
        Read::Mangled { .. } => held(facts, item, Hold::Record, Behind::NOTHING),
        Read::Record(Lifecycle { phase, .. }) => match phase {
            Phase::Waiting | Phase::Parked => ask(entry, id, out),
            Phase::Retrying(class) => State::Backoff { until: backoff(entry.failures, class, env, rng) },
            Phase::Claimed => {
                out.push(Request::Adopt { item, attempt: entry.attempts });
                State::Running { stopped: false }
            }
            Phase::Applying { outcome } => apply(entry, id, outcome, false, out),
            Phase::Held { why, outcome } => held(facts, item, why, Behind { ack: None, outcome }),
            Phase::Done => unreachable!("refused above: a done item is not live"),
        },
    };
    follow(domain, id, out);
}

pub(crate) fn stop(domain: &mut Domain, reply_to: ReplyTo, item: Item, out: &mut Queue<Request>) {
    let Some(id) = named(domain, item) else {
        out.push(Request::Refused { to: reply_to, refusal: Refusal::Unknown });
        return;
    };
    let entry = domain.tracked.get_mut(id).expect("a named item is tracked");
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Claiming { run, stopped: _ } => {
            out.push(Request::Stopped { to: reply_to });
            State::Claiming { run, stopped: true }
        }
        State::Running { stopped } => cancel(entry, reply_to, stopped, out),
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
    follow(domain, id, out);
}

pub(crate) fn release(domain: &mut Domain, reply_to: ReplyTo, item: Item, out: &mut Queue<Request>) {
    let Some(id) = named(domain, item) else {
        out.push(Request::Refused { to: reply_to, refusal: Refusal::Unknown });
        return;
    };
    let Domain { tracked, facts, .. } = domain;
    let entry = tracked.get_mut(id).expect("a named item is tracked");
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Held { why: _, behind } => released(entry, id, facts, reply_to, behind, out),
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
    follow(domain, id, out);
}

pub(crate) fn inbox(
    domain: &mut Domain,
    env: &Env<Limits>,
    item: Item,
    event: Token,
    wake: Option<Time>,
    out: &mut Queue<Request>,
) {
    let Some(id) = named(domain, item) else {
        return;
    };
    let entry = domain.tracked.get_mut(id).expect("a named item is tracked");
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Asking { wake: earlier } => State::Asking { wake: earliest(earlier, wake) },
        State::Waiting { until } => woken(entry, id, env, until, wake, out),
        State::Running { stopped } => {
            if !stopped {
                out.push(Request::Relay { item, attempt: entry.attempts, event });
            }
            State::Running { stopped }
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
    follow(domain, id, out);
}

pub(crate) fn placed(domain: &mut Domain, item: Item, attempt: u64, out: &mut Queue<Request>) {
    let Some(id) = named(domain, item) else {
        return;
    };
    let Domain { tracked, facts, .. } = domain;
    let entry = tracked.get_mut(id).expect("a named item is tracked");
    let in_flight = match entry.state {
        State::Running { stopped } => attempt == entry.attempts && !stopped,
        State::Writing { .. }
        | State::Claiming { .. }
        | State::Asking { .. }
        | State::Waiting { .. }
        | State::Recording { .. }
        | State::Applying { .. }
        | State::Acting { .. }
        | State::Backoff { .. }
        | State::Held { .. }
        | State::Closed => false,
    };
    if !in_flight {
        // The fleet tells only of the attempt in flight; a stopped one goes
        // on to its answer without what it missed.
        return;
    }
    facts.push(Fact::Placed { item, attempt });
    entry.refusals = 0;
    for _ in 0..entry.undelivered.len() {
        let Some(event) = entry.undelivered.pop() else { break };
        out.push(Request::Relay { item, attempt, event });
    }
}

pub(crate) fn undelivered(domain: &mut Domain, item: Item, attempt: u64, event: Token) {
    let Some(id) = named(domain, item) else {
        return;
    };
    let entry = domain.tracked.get_mut(id).expect("a named item is tracked");
    let in_flight = match entry.state {
        State::Running { stopped } => attempt == entry.attempts && !stopped,
        State::Writing { .. }
        | State::Claiming { .. }
        | State::Asking { .. }
        | State::Waiting { .. }
        | State::Recording { .. }
        | State::Applying { .. }
        | State::Acting { .. }
        | State::Backoff { .. }
        | State::Held { .. }
        | State::Closed => false,
    };
    if in_flight {
        // One that does not fit stays in the item's inbox, for the next run.
        let _kept: Result<(), Token> = entry.undelivered.try_push(event);
    }
}

pub(crate) fn listed(domain: &mut Domain, item: Item, attempt: u64) {
    let Some(id) = named(domain, item) else {
        return;
    };
    let entry = domain.tracked.get_mut(id).expect("a named item is tracked");
    match entry.state {
        State::Held { why: Hold::Record, .. } => entry.attempts = entry.attempts.max(attempt),
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

pub(crate) fn answered(
    domain: &mut Domain,
    env: &Env<Limits>,
    item: Item,
    attempt: u64,
    answer: Answer,
    out: &mut Queue<Request>,
) {
    let Some(id) = named(domain, item) else {
        out.push(Request::Stale { item, attempt });
        return;
    };
    let Domain { tracked, facts, .. } = domain;
    let entry = tracked.get_mut(id).expect("a named item is tracked");
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Running { stopped } if attempt == entry.attempts => {
            // The run is over: what it missed stays in the item's inbox.
            forget(&mut entry.undelivered);
            match answer {
                Answer::Ended { outcome } => ended(entry, id, facts, outcome, stopped, out),
                Answer::Parked { snapshot } => parked(entry, id, facts, snapshot, stopped, out),
                Answer::Failed(class) => failed(entry, id, env, facts, class, Behind::ack(attempt), stopped, out),
                Answer::Lost => failed(entry, id, env, facts, Class::Lost, Behind::NOTHING, stopped, out),
                Answer::Refused => refused(entry, id, facts, stopped, out),
            }
        }
        // A copy of the answer being made durable: its acknowledgement
        // follows once it is, and its worker keeps it until then.
        State::Recording { stopped } if attempt == entry.attempts => State::Recording { stopped },
        State::Writing { next, behind } if behind.ack == Some(attempt) => State::Writing { next, behind },
        State::Held { why, behind } if behind.ack == Some(attempt) => State::Held { why, behind },
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
    follow(domain, id, out);
}

pub(crate) fn decided(domain: &mut Domain, owner: Token, due: Due, out: &mut Queue<Request>) {
    let id = Id::from_token(owner);
    let Domain { tracked, facts, .. } = domain;
    let entry = tracked.get_mut(id).expect("a token travelling up is never stale");
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Asking { wake } => match due {
            Due::Nothing { until } => nothing(wake, until),
            Due::Run { run } => claim(entry, id, facts, run, out),
            Due::Act { action } => act(entry, id, action, false, out),
            Due::Done { action } => act(entry, id, action, true, out),
            Due::Hold { reason } => hold(entry, id, Hold::Plan { reason }, None, Behind::NOTHING, out),
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
    follow(domain, id, out);
}

pub(crate) fn written(domain: &mut Domain, env: &Env<Limits>, owner: Token, wrote: Wrote, out: &mut Queue<Request>) {
    let id = Id::from_token(owner);
    let Domain { tracked, rng, facts, .. } = domain;
    let entry = tracked.get_mut(id).expect("a token travelling up is never stale");
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Writing { next, behind } => match wrote {
            Wrote::Done => {
                if let Some(attempt) = behind.ack {
                    out.push(Request::Acknowledge { item: entry.item, attempt });
                }
                proceed(entry, id, env, rng, facts, next, out)
            }
            // Not on the forge: the answer it would have made durable is not
            // acknowledged, and its worker keeps it.
            Wrote::Failed => held(facts, entry.item, Hold::Record, behind),
        },
        State::Claiming { run, stopped } => match wrote {
            Wrote::Done => claimed(entry, id, run, stopped, out),
            Wrote::Failed => held(facts, entry.item, Hold::Record, Behind::NOTHING),
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
    follow(domain, id, out);
}

pub(crate) fn recorded(domain: &mut Domain, owner: Token, comment: Option<u64>, out: &mut Queue<Request>) {
    let id = Id::from_token(owner);
    let entry = domain.tracked.get_mut(id).expect("a token travelling up is never stale");
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Recording { stopped } => {
            let attempt = entry.attempts;
            match comment {
                Some(outcome) => {
                    let next = Next::Apply { outcome, stopped };
                    let behind = Behind { ack: Some(attempt), outcome: Some(outcome) };
                    writing(entry, id, Phase::Applying { outcome }, next, behind, out)
                }
                // Not posted: nothing of the outcome is on the forge to keep.
                None => hold(entry, id, Hold::Writes, None, Behind::ack(attempt), out),
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
    follow(domain, id, out);
}

pub(crate) fn applied(
    domain: &mut Domain,
    env: &Env<Limits>,
    owner: Token,
    applied: Applied,
    out: &mut Queue<Request>,
) {
    let id = Id::from_token(owner);
    let Domain { tracked, facts, .. } = domain;
    let entry = tracked.get_mut(id).expect("a token travelling up is never stale");
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Applying { outcome, stopped } => {
            let behind = Behind::outcome(outcome);
            match applied {
                Applied::Made(then) => made(entry, id, facts, outcome, then, stopped, out),
                Applied::Stale => stale(entry, id, facts, outcome, stopped, out),
                Applied::Invalid => failed(entry, id, env, facts, Class::Invalid, behind, stopped, out),
                Applied::Accepting => hold(entry, id, Hold::Acceptance, Some(outcome), behind, out),
                Applied::Failed => hold(entry, id, Hold::Writes, Some(outcome), behind, out),
            }
        }
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
    follow(domain, id, out);
}

pub(crate) fn acted(domain: &mut Domain, owner: Token, acted: Acted, out: &mut Queue<Request>) {
    let id = Id::from_token(owner);
    let Domain { tracked, facts, .. } = domain;
    let entry = tracked.get_mut(id).expect("a token travelling up is never stale");
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Acting { done } => match acted {
            Acted::Made => {
                facts.push(Fact::Applied { item: entry.item });
                if done {
                    writing(entry, id, Phase::Done, Next::Leave, Behind::NOTHING, out)
                } else {
                    writing(entry, id, Phase::Waiting, Next::Ask, Behind::NOTHING, out)
                }
            }
            Acted::Stale => {
                facts.push(Fact::Stale { item: entry.item });
                ask(entry, id, out)
            }
            Acted::Accepting => hold(entry, id, Hold::Acceptance, None, Behind::NOTHING, out),
            Acted::Failed => hold(entry, id, Hold::Writes, None, Behind::NOTHING, out),
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
    follow(domain, id, out);
}

pub(crate) fn alarm(domain: &mut Domain, id: Id<Tracked>, out: &mut Queue<Request>) {
    let entry = domain.tracked.get_mut(id).expect("an item's alarm is cancelled as it closes");
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Waiting { .. } | State::Backoff { .. } => ask(entry, id, out),
        State::Writing { .. }
        | State::Claiming { .. }
        | State::Asking { .. }
        | State::Running { .. }
        | State::Recording { .. }
        | State::Applying { .. }
        | State::Acting { .. }
        | State::Held { .. }
        | State::Closed => unreachable!("an alarm is armed only while waiting or backing off"),
    };
    follow(domain, id, out);
}

fn named(domain: &Domain, item: Item) -> Option<Id<Tracked>> {
    domain.names.get(&item).copied()
}

/// Derives what the item's state implies, after every transition: its alarm,
/// and, once it is Closed, its retirement.
fn follow(domain: &mut Domain, id: Id<Tracked>, out: &mut Queue<Request>) {
    let Domain { tracked, names, alarms, facts, .. } = domain;
    let entry = tracked.get(id).expect("an item lives until it is retired");
    let (alarm, closed) = match &entry.state {
        State::Waiting { until } => (*until, false),
        State::Backoff { until } => (Some(*until), false),
        State::Closed => (None, true),
        State::Writing { .. }
        | State::Claiming { .. }
        | State::Asking { .. }
        | State::Running { .. }
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
/// is written, leaving `behind` what the write leaves.
fn writing(
    entry: &Tracked,
    id: Id<Tracked>,
    phase: Phase,
    next: Next,
    behind: Behind,
    out: &mut Queue<Request>,
) -> State {
    let lifecycle = Lifecycle { phase, attempts: entry.attempts, failures: entry.failures };
    out.push(Request::Write { owner: id.token(), item: entry.item, lifecycle });
    State::Writing { next, behind }
}

/// Writes the record held for `why`, keeping `outcome`, and holds the item
/// once it is written; one that cannot be is held all the same, keeping that
/// outcome too.
fn hold(
    entry: &Tracked,
    id: Id<Tracked>,
    why: Hold,
    outcome: Option<u64>,
    behind: Behind,
    out: &mut Queue<Request>,
) -> State {
    let behind = Behind { ack: behind.ack, outcome: behind.outcome.or(outcome) };
    writing(entry, id, Phase::Held { why, outcome }, Next::Held { why, outcome }, behind, out)
}

/// The item is held, as its record says, or as it cannot, keeping what the
/// hold leaves `behind`.
fn held(facts: &mut Facts, item: Item, why: Hold, behind: Behind) -> State {
    facts.push(Fact::Held { item, why });
    State::Held { why, behind }
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
        Next::Pause => State::Backoff { until: pause(entry.refusals, env, rng) },
        Next::Held { why, outcome } => held(facts, entry.item, why, Behind { ack: None, outcome }),
        Next::Leave => State::Closed,
    }
}

/// Claiming, written: the claim is on the forge, so the run starts, unless a
/// person stopped it meanwhile.
fn claimed(entry: &Tracked, id: Id<Tracked>, run: Token, stopped: bool, out: &mut Queue<Request>) -> State {
    if stopped {
        return hold(entry, id, Hold::Stopped, None, Behind::NOTHING, out);
    }
    out.push(Request::Start { item: entry.item, attempt: entry.attempts, run });
    State::Running { stopped: false }
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
fn cancel(entry: &mut Tracked, reply_to: ReplyTo, stopped: bool, out: &mut Queue<Request>) -> State {
    out.push(Request::Stopped { to: reply_to });
    if !stopped {
        out.push(Request::Cancel { item: entry.item, attempt: entry.attempts });
    }
    // What it missed goes no further.
    forget(&mut entry.undelivered);
    State::Running { stopped: true }
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
    entry.refusals = 0;
    if stopped {
        return hold(entry, id, Hold::Stopped, None, Behind::ack(attempt), out);
    }
    writing(entry, id, Phase::Parked, Next::Ask, Behind::ack(attempt), out)
}

/// Running, answered: the run failed in `class` (or was lost, with nothing
/// to acknowledge); so did an outcome that is invalid, which the write
/// leaves `behind`. It is retried after a backoff, as often as the class
/// allows, and then held; a stopped one is held at once.
#[expect(clippy::too_many_arguments, reason = "a cell handler takes what its cell touches")]
fn failed(
    entry: &mut Tracked,
    id: Id<Tracked>,
    env: &Env<Limits>,
    facts: &mut Facts,
    class: Class,
    behind: Behind,
    stopped: bool,
    out: &mut Queue<Request>,
) -> State {
    facts.push(Fact::Failed { item: entry.item, attempt: entry.attempts, class });
    entry.refusals = 0;
    if stopped {
        return hold(entry, id, Hold::Stopped, None, behind, out);
    }
    entry.failures = entry.failures.and(class);
    if entry.failures.of(class) > env.limits.retries.of(class).retries {
        return hold(entry, id, Hold::Failures(class), None, behind, out);
    }
    writing(entry, id, Phase::Retrying(class), Next::Backoff { class }, behind, out)
}

/// Running, answered: nothing ran. No failure: the record goes back to
/// waiting, and the item claims again after a pause, longer for each refusal
/// in a row; a stopped one is held.
fn refused(entry: &mut Tracked, id: Id<Tracked>, facts: &mut Facts, stopped: bool, out: &mut Queue<Request>) -> State {
    facts.push(Fact::Refused { item: entry.item, attempt: entry.attempts });
    if stopped {
        return hold(entry, id, Hold::Stopped, None, Behind::NOTHING, out);
    }
    entry.refusals = entry.refusals.saturating_add(1);
    writing(entry, id, Phase::Waiting, Next::Pause, Behind::NOTHING, out)
}

/// Applying, applied: the outcome's writes are made. The record says what
/// the item does next, the commit point.
fn made(
    entry: &mut Tracked,
    id: Id<Tracked>,
    facts: &mut Facts,
    outcome: u64,
    then: Then,
    stopped: bool,
    out: &mut Queue<Request>,
) -> State {
    facts.push(Fact::Applied { item: entry.item });
    entry.failures = Failures::NONE;
    let behind = Behind::outcome(outcome);
    if stopped {
        return hold(entry, id, Hold::Stopped, None, behind, out);
    }
    match then {
        Then::Wait => writing(entry, id, Phase::Waiting, Next::Ask, behind, out),
        Then::Hold { reason } => hold(entry, id, Hold::Plan { reason }, None, behind, out),
    }
}

/// Applying, applied: the outcome is stale, and nothing of it is made. The
/// run did not fail: the item moved on.
fn stale(
    entry: &mut Tracked,
    id: Id<Tracked>,
    facts: &mut Facts,
    outcome: u64,
    stopped: bool,
    out: &mut Queue<Request>,
) -> State {
    facts.push(Fact::Stale { item: entry.item });
    entry.failures = Failures::NONE;
    let behind = Behind::outcome(outcome);
    if stopped {
        return hold(entry, id, Hold::Stopped, None, behind, out);
    }
    writing(entry, id, Phase::Waiting, Next::Ask, behind, out)
}

/// Held, release: the outcome the hold keeps is applied again; an item that
/// keeps none is due again, with its failures forgiven. An answer it has not
/// acknowledged is, once the release's record is written.
fn released(
    entry: &mut Tracked,
    id: Id<Tracked>,
    facts: &mut Facts,
    reply_to: ReplyTo,
    behind: Behind,
    out: &mut Queue<Request>,
) -> State {
    out.push(Request::Released { to: reply_to });
    facts.push(Fact::Released { item: entry.item });
    entry.refusals = 0;
    match behind.outcome {
        Some(outcome) => {
            let next = Next::Apply { outcome, stopped: false };
            writing(entry, id, Phase::Applying { outcome }, next, behind, out)
        }
        None => {
            entry.failures = Failures::NONE;
            writing(entry, id, Phase::Waiting, Next::Ask, behind, out)
        }
    }
}

/// Forgets the inbound events kept for a run that will not take them: they
/// stay in the item's inbox, for the next run.
fn forget(undelivered: &mut Queue<Token>) {
    for _ in 0..undelivered.len() {
        let _event: Option<Token> = undelivered.pop();
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

/// When to retry an item whose failures are `failures`, the last in `class`.
fn backoff(failures: Failures, class: Class, env: &Env<Limits>, rng: &mut Rng) -> Time {
    let retry = env.limits.retries.of(class);
    jittered(failures.of(class), retry.base, retry.max, env, rng)
}

/// When to claim again after `refusals` runs refused in a row: as a
/// transient failure is retried, but counting none.
fn pause(refusals: u32, env: &Env<Limits>, rng: &mut Rng) -> Time {
    let retry = env.limits.retries.transient;
    jittered(refusals, retry.base, retry.max, env, rng)
}

/// `env.now` and a backoff exponential in `times` and capped at `max`, with
/// equal jitter: half fixed, half random.
fn jittered(times: u32, base: Duration, max: Duration, env: &Env<Limits>, rng: &mut Rng) -> Time {
    let doublings = times.saturating_sub(1);
    let factor = 1_u64.checked_shl(doublings).unwrap_or(u64::MAX);
    let ceiling = base.saturating_mul(factor).min(max);
    let half = ceiling.as_nanos() / 2;
    let jittered = Duration::from_nanos(half.saturating_add(rng.below(half.saturating_add(1))));
    env.now.saturating_add(jittered)
}
