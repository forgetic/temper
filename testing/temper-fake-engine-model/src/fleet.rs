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
//! A hello lists what the worker hosts. Each listed attempt the engine has
//! out is kept, by the `keeps` chance, or cancelled; one it had cancelled is
//! cancelled again, and one it presumed lost is cancelled, its item having
//! moved on. Each attempt out on the worker that it does not list is presumed
//! lost.

use core::mem;

use temper_lib::{Env, Id, Map, Set, Token};

use crate::api::Hello;
use crate::model::{Alarm, Config, Model};
use crate::work::{self, Attempted};

/// A worker that has said hello.
#[derive(Debug)]
pub(crate) struct Worker {
    /// How many runs it hosts at once, as it last said.
    slots: u32,
    /// Its attempts not answered yet, those presumed lost aside.
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
    let mut heard = Set::with_capacity(u32::try_from(hosting.len()).expect("a worker's runs fit a u32"));
    for hosted in &hosting {
        work::check_made(model.made, hosted.attempt);
        let record =
            model.attempts.get_mut(&hosted.attempt).expect("a worker hosts only attempts that have not answered");
        assert!(record.item == Id::from_token(hosted.run), "a hosted attempt is of its run");
        assert!(record.worker == worker, "a worker hosts only the attempts it was given");
        assert!(heard.insert(hosted.attempt) == Ok(true), "a worker lists each attempt once");
        let cancel = match record.state {
            Attempted::Live => {
                let cancel = !model.rng.chance(config.keeps);
                if cancel {
                    record.state = Attempted::Cancelled;
                    model.timers.cancel(Alarm::Cancel(hosted.attempt));
                }
                cancel
            }
            Attempted::Cancelled | Attempted::Lost => true,
        };
        if cancel {
            assert!(model.cancels.insert(hosted.attempt).is_ok(), "room for a cancel per attempt");
        }
    }
    let record = model.workers.get_mut(&worker).expect("a worker is known once it said hello");
    record.slots = slots;
    record.contact = Contact::Up;
    let placed = mem::replace(&mut record.placed, Set::with_capacity(config.items));
    let mut kept = Set::with_capacity(config.items);
    for &attempt in &placed {
        if heard.contains(&attempt) {
            assert!(kept.insert(attempt).is_ok(), "room for an attempt per item");
        } else {
            work::lose(model, env, attempt);
        }
    }
    model.workers.get_mut(&worker).expect("a worker is known once it said hello").placed = kept;
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

/// Lost, grace: every attempt out on the worker is presumed lost.
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
