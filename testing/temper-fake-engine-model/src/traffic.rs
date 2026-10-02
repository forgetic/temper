//! What the engine sends live runs besides their assignments, and the calls
//! they relay to it.
//!
//! - **Inbound events** go to a live attempt at drawn moments, up to the
//!   configured number; one bounced for want of room may be sent again. A
//!   cancelled attempt gets none.
//! - **Cancels** go to some attempts at a drawn moment, and again to those a
//!   hello lists but the engine no longer keeps. Some go late, to attempts
//!   that have answered; and some inbound events go to attempts their items
//!   have replaced. A worker must drop both.
//! - **Relayed calls** from a live attempt are answered once, after a drawn
//!   latency, with an opaque body or an error, even if the attempt has
//!   answered meanwhile. Those of an attempt the engine has cancelled or
//!   presumed lost are dropped as they come (engine-model.md, section 8),
//!   and so are those whose worker is out of contact past the grace when
//!   their answer falls due.
//!
//! Nothing is sent to a worker out of contact: an alarm that falls due
//! meanwhile fires again later, while its attempt is still out; traffic for
//! attempts over and done is dropped.
//!
//! A relayed call's transition table:
//!
//! ```text
//! state    event or alarm             next     emits
//! Waiting  its alarm, worker up       Closed   relayed
//!          its alarm, worker lost     Waiting  (the alarm again)
//!          its alarm, worker gone     Closed
//! ```

use core::mem;

use temper_lib::{Env, Id, Queue, Token};

use crate::api::{Bounce, Reply};
use crate::charter;
use crate::fleet::{self, Contact};
use crate::model::{Alarm, Config, Model, Request};
use crate::work::{self, Attempted, after};

/// What an inbound event says, over and over.
const EVENT: &[u8] = b"A person says: please also update the changelog. ";

/// What a relayed call's answer says, over and over.
const ANSWER: &[u8] = b"Issue 42: the parser fails on empty input. ";

/// A relayed call being answered.
#[derive(Debug)]
pub(crate) enum Call {
    /// The call `call` of the run `run`'s attempt `attempt` on `worker`,
    /// answered when its alarm fires.
    Waiting { worker: Token, run: Token, attempt: Token, call: Token },
    /// Terminal: holds nothing.
    Closed,
}

/// The attempt's inbound alarm: an event, if it is live and its worker in
/// contact.
pub(crate) fn inbound(model: &mut Model, env: &Env<Config>, attempt: Token, out: &mut Queue<Request>) {
    let config = &env.limits;
    let Model { attempts, workers, timers, rng, tally, .. } = model;
    let record = attempts.get_mut(&attempt).expect("an attempt's alarms stop once it answers");
    match record.state {
        Attempted::Live => {}
        Attempted::Cancelled => return,
        Attempted::Lost => unreachable!("an attempt's alarms stop once it is lost"),
    }
    let send = match fleet::contact(workers, record.worker) {
        Contact::Up => {
            let event = charter::text(EVENT, rng.between(u64::from(config.event_min), u64::from(config.event_max)));
            out.push(Request::Inbound { worker: record.worker, run: record.item.token(), attempt, event });
            record.sent = record.sent.saturating_add(1);
            tally.inbound = tally.inbound.saturating_add(1);
            record.sent < config.inbound
        }
        Contact::Lost => true,
        Contact::Gone => unreachable!("an attempt on a worker gone is lost"),
    };
    if send {
        let at = after(env, rng.between(config.inbound_min.as_nanos(), config.inbound_max.as_nanos()));
        timers.arm(Alarm::Inbound(attempt), at).expect("a timer per attempt");
    }
}

/// The attempt's cancel alarm: Live, cancel.
pub(crate) fn cancel(model: &mut Model, env: &Env<Config>, attempt: Token, out: &mut Queue<Request>) {
    let config = &env.limits;
    let Model { attempts, workers, timers, rng, tally, .. } = model;
    let record = attempts.get_mut(&attempt).expect("an attempt's alarms stop once it answers");
    record.state = match mem::replace(&mut record.state, Attempted::Lost) {
        Attempted::Live => match fleet::contact(workers, record.worker) {
            Contact::Up => {
                out.push(Request::Cancel { worker: record.worker, run: record.item.token(), attempt });
                tally.cancels = tally.cancels.saturating_add(1);
                Attempted::Cancelled
            }
            Contact::Lost => {
                let at = after(env, rng.between(config.cancel_min.as_nanos(), config.cancel_max.as_nanos()));
                timers.arm(Alarm::Cancel(attempt), at).expect("a timer per attempt");
                Attempted::Live
            }
            Contact::Gone => unreachable!("an attempt on a worker gone is lost"),
        },
        Attempted::Cancelled | Attempted::Lost => {
            unreachable!("an attempt's cancel alarm stops once it is cancelled or lost")
        }
    };
}

/// From the ready list: cancels an attempt a hello listed, if it has not
/// answered since and its worker is still in contact.
pub(crate) fn recancel(model: &mut Model, attempt: Token, out: &mut Queue<Request>) {
    let Some(record) = model.attempts.get(&attempt) else {
        return;
    };
    match fleet::contact(&model.workers, record.worker) {
        Contact::Up => {
            out.push(Request::Cancel { worker: record.worker, run: record.item.token(), attempt });
            model.tally.cancels = model.tally.cancels.saturating_add(1);
        }
        Contact::Lost | Contact::Gone => {}
    }
}

/// A late cancel, for an attempt that has answered.
pub(crate) fn late(model: &mut Model, worker: Token, run: Token, attempt: Token, out: &mut Queue<Request>) {
    match fleet::contact(&model.workers, worker) {
        Contact::Up => {
            out.push(Request::Cancel { worker, run, attempt });
            model.tally.late_cancels = model.tally.late_cancels.saturating_add(1);
        }
        Contact::Lost | Contact::Gone => {}
    }
}

/// An inbound event for an attempt its item has replaced.
pub(crate) fn stale(
    model: &mut Model,
    env: &Env<Config>,
    worker: Token,
    run: Token,
    attempt: Token,
    out: &mut Queue<Request>,
) {
    let config = &env.limits;
    match fleet::contact(&model.workers, worker) {
        Contact::Up => {
            let len = model.rng.between(u64::from(config.event_min), u64::from(config.event_max));
            out.push(Request::Inbound { worker, run, attempt, event: charter::text(EVENT, len) });
            model.tally.stale = model.tally.stale.saturating_add(1);
        }
        Contact::Lost | Contact::Gone => {}
    }
}

/// An inbound event was bounced: one bounced for want of room may be sent
/// again, while its attempt is live.
pub(crate) fn bounced(model: &mut Model, env: &Env<Config>, run: Token, attempt: Token, bounce: Bounce) {
    let config = &env.limits;
    work::check_made(model.made, attempt);
    let record = model.attempts.get(&attempt).expect("an attempt bounces events before it answers");
    assert!(record.item == Id::from_token(run), "a bounce names its attempt's run");
    model.tally.bounced = model.tally.bounced.saturating_add(1);
    match bounce {
        Bounce::Full => {}
        Bounce::TooLarge | Bounce::Ending => return,
    }
    match record.state {
        Attempted::Live => {}
        Attempted::Cancelled | Attempted::Lost => return,
    }
    if model.rng.chance(config.resends) {
        let at = after(env, model.rng.between(config.inbound_min.as_nanos(), config.inbound_max.as_nanos()));
        model.timers.arm(Alarm::Inbound(attempt), at).expect("a timer per attempt");
        model.tally.resent = model.tally.resent.saturating_add(1);
    }
}

/// A relayed call: answered later if its attempt is live, else dropped.
pub(crate) fn relay(model: &mut Model, env: &Env<Config>, run: Token, attempt: Token, call: Token) {
    let config = &env.limits;
    work::check_made(model.made, attempt);
    let record = model.attempts.get(&attempt).expect("an attempt relays calls before it answers");
    assert!(record.item == Id::from_token(run), "a call names its attempt's run");
    model.tally.relayed = model.tally.relayed.saturating_add(1);
    match record.state {
        Attempted::Live => {}
        Attempted::Cancelled | Attempted::Lost => {
            model.tally.dropped = model.tally.dropped.saturating_add(1);
            return;
        }
    }
    assert!(model.named.insert((attempt, call)) == Ok(true), "a run names its calls in flight apart");
    let waiting = Call::Waiting { worker: record.worker, run, attempt, call };
    let id = model.calls.insert(waiting).expect("room for every call in flight");
    let at = after(env, model.rng.between(config.relay_min.as_nanos(), config.relay_max.as_nanos()));
    model.timers.arm(Alarm::Call(id), at).expect("a timer per call");
}

/// A relayed call's alarm: Waiting, answered once its worker is in contact.
pub(crate) fn reply(model: &mut Model, env: &Env<Config>, id: Id<Call>, out: &mut Queue<Request>) {
    let config = &env.limits;
    let Model { calls, named, workers, timers, rng, tally, .. } = model;
    let pending = calls.get_mut(id).expect("a call lives until its alarm closes it");
    *pending = match mem::replace(pending, Call::Closed) {
        Call::Waiting { worker, run, attempt, call } => match fleet::contact(workers, worker) {
            Contact::Up => {
                let reply = if rng.chance(config.relay_errors) {
                    tally.errors = tally.errors.saturating_add(1);
                    Reply::Error
                } else {
                    let len = rng.between(u64::from(config.answer_min), u64::from(config.answer_max));
                    Reply::Answer { body: charter::text(ANSWER, len) }
                };
                out.push(Request::Relayed { worker, run, attempt, call, reply });
                tally.replies = tally.replies.saturating_add(1);
                named.remove(&(attempt, call));
                Call::Closed
            }
            Contact::Lost => {
                let at = after(env, rng.between(config.relay_min.as_nanos(), config.relay_max.as_nanos()));
                timers.arm(Alarm::Call(id), at).expect("a timer per call");
                Call::Waiting { worker, run, attempt, call }
            }
            Contact::Gone => {
                tally.dropped = tally.dropped.saturating_add(1);
                named.remove(&(attempt, call));
                Call::Closed
            }
        },
        Call::Closed => unreachable!("a closed call has no alarm"),
    };
    let closed = match pending {
        Call::Closed => true,
        Call::Waiting { .. } => false,
    };
    if closed {
        calls.retire(id);
    }
}
