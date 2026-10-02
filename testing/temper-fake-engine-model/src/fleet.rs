//! The workers (engine-model.md, section 8): each dials in with a hello,
//! saying how many runs it hosts at once and, on a reconnect, which runs it
//! still hosts. Placement takes the first worker in contact with a free slot,
//! by the world's names for them, preferring none for its workstreams yet.
//!
//! A worker's contact:
//!
//! ```text
//! state  event or alarm  next  emits
//! Up     lost            Lost  (the grace alarm)
//! Up     hello           Up    (a reconnect the engine had not noticed)
//! Lost   hello           Up
//! Lost   grace           Gone  (its attempts lost)
//! Gone   hello           Up
//! ```
//!
//! A hello lists what the worker hosts, each run with its phase. A run whose
//! answer the worker holds, answered while the channel was down, is kept: the
//! answer follows the hello. Of the others, an attempt the engine has out is
//! kept, by the `keeps` chance, or cancelled; one it had cancelled is
//! cancelled again, the first cancel having maybe been lost, and one it
//! presumed lost is cancelled, its item having moved on. A listed attempt
//! takes one of the worker's slots until it answers, a lost one too. Each
//! attempt out on the worker that it does not list is presumed lost, and a
//! listing of an attempt that has answered is counted and dropped.

use core::mem;

use temper_lib::{Env, Map, Set, Token};

use crate::api::{Hello, Hosted, Phase};
use crate::model::{Alarm, Config, Model};
use crate::work::{self, Attempted};

/// A worker that has said hello.
#[derive(Debug)]
pub(crate) struct Worker {
    /// How many runs it hosts at once, as it last said.
    slots: u32,
    /// The attempts it holds, as far as the engine knows: those out on it and
    /// not answered yet, and those presumed lost that it listed since.
    pub(crate) placed: Set<Token>,
    pub(crate) contact: Contact,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Contact {
    Up,
    /// Its channel dropped: its grace alarm is armed.
    Lost,
    /// Out of contact past the grace: its attempts are presumed lost.
    Gone,
}

/// The first worker in contact with a free slot.
pub(crate) fn free(workers: &Map<Token, Worker>) -> Option<Token> {
    for (&token, worker) in workers {
        match worker.contact {
            Contact::Up if worker.placed.len() < worker.slots => return Some(token),
            Contact::Up | Contact::Lost | Contact::Gone => {}
        }
    }
    None
}

/// The first worker in contact.
pub(crate) fn up(workers: &Map<Token, Worker>) -> Option<Token> {
    for (&token, worker) in workers {
        match worker.contact {
            Contact::Up => return Some(token),
            Contact::Lost | Contact::Gone => {}
        }
    }
    None
}

/// The contact of `worker`, which has said hello.
pub(crate) fn contact(workers: &Map<Token, Worker>, worker: Token) -> Contact {
    workers.get(&worker).expect("a worker is known once it said hello").contact
}

/// Any state, hello: the worker is in contact, and what it hosts is kept,
/// cancelled or lost.
pub(crate) fn hello(model: &mut Model, env: &Env<Config>, worker: Token, hello: Hello) {
    let config = &env.limits;
    let Hello { slots, workstreams: _, hosting } = hello;
    if !model.workers.contains_key(&worker) {
        let fresh = Worker { slots, placed: Set::with_capacity(config.items), contact: Contact::Up };
        let room = model.workers.insert(worker, fresh);
        assert!(room.is_ok(), "room for every worker the world has");
    }
    model.timers.cancel(Alarm::Grace(worker));
    let mut heard = Set::with_capacity(config.items);
    for &Hosted { run, attempt, phase } in &hosting {
        let Some(state) = work::state(model, worker, run, attempt) else {
            model.tally.fenced = model.tally.fenced.saturating_add(1);
            continue;
        };
        let fresh = heard.insert(attempt).expect("a worker hosts one attempt of a run at a time");
        assert!(fresh, "a worker lists each attempt once");
        match phase {
            Phase::Answered => continue,
            Phase::Preparing | Phase::Starting | Phase::Active | Phase::Waiting | Phase::Ending => {}
        }
        match state {
            Attempted::Live => {
                if model.rng.chance(config.keeps) {
                    continue;
                }
                model.attempts.get_mut(&attempt).expect("a listed attempt has not answered").state =
                    Attempted::Cancelled;
                model.timers.cancel(Alarm::Cancel(attempt));
            }
            Attempted::Cancelled | Attempted::Lost => {}
        }
        assert!(model.cancels.insert(attempt).is_ok(), "room for a cancel per attempt");
    }
    let record = model.workers.get_mut(&worker).expect("a worker is known once it said hello");
    record.slots = slots;
    record.contact = Contact::Up;
    let placed = mem::replace(&mut record.placed, Set::with_capacity(config.items));
    for &attempt in &placed {
        if !heard.contains(&attempt) {
            work::lose(model, env, attempt);
        }
    }
    model.workers.get_mut(&worker).expect("a worker is known once it said hello").placed = heard;
    model.tally.hellos = model.tally.hellos.saturating_add(1);
}

/// Up, lost: the worker keeps its runs for the grace.
pub(crate) fn lost(model: &mut Model, env: &Env<Config>, worker: Token) {
    let record = model.workers.get_mut(&worker).expect("only a worker that said hello loses contact");
    record.contact = match record.contact {
        Contact::Up => {
            let at = env.now.saturating_add(env.limits.grace);
            model.timers.arm(Alarm::Grace(worker), at).expect("a timer per worker");
            Contact::Lost
        }
        Contact::Lost => Contact::Lost,
        Contact::Gone => Contact::Gone,
    };
}

/// Lost, grace: every attempt out on the worker is presumed lost, and it
/// holds none the engine knows of.
pub(crate) fn grace(model: &mut Model, env: &Env<Config>, worker: Token) {
    let config = &env.limits;
    let record = model.workers.get_mut(&worker).expect("a worker is known once it said hello");
    record.contact = match record.contact {
        Contact::Lost => Contact::Gone,
        Contact::Up | Contact::Gone => unreachable!("the grace runs while contact is lost"),
    };
    let placed = mem::replace(&mut record.placed, Set::with_capacity(config.items));
    for &attempt in &placed {
        work::lose(model, env, attempt);
    }
}
