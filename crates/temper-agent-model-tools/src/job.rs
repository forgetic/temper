//! Jobs: the calls a kit is running, each waiting on one operation of io.
//!
//! A call that passes the kit's entrance becomes a job, which asks io for its
//! operation and answers the call once that operation has ended. Every
//! operation ends in exactly one terminal event, so a job answers exactly once,
//! whether the operation completed, failed, timed out, or lost or won a race
//! with a cancel.
//!
//! A write stores the file if it is as the kit knows it: at the version its
//! LLM read or wrote last, or absent if it knows none. io checks that against
//! the real file as it stores, so a change made since, by another kit or by
//! anything else, is caught: as `Stale` if the LLM had read the file, and as
//! `NotRead` if it had not and the write would have created it. A write that
//! may or may not have happened (timed out, failed) leaves the knowledge as
//! it was, and the next write's check settles it.
//!
//! The transition table. Every other cell is unreachable by the boundary's
//! contract: an operation ends only in its own terminals.
//!
//! ```text
//! state      terminal                    next   answer
//! Reading    loaded                      Done   the window; the version is known
//!            missing                     Done   not found; nothing is known there
//!            any other                   Done   what it says
//! Listing    scanned                     Done   the entries
//!            any other                   Done   what it says
//! Writing    stored                      Done   written; the new version is known
//!            conflict, creating          Done   not read
//!            conflict, replacing         Done   stale; a file now absent is forgotten
//!            any other                   Done   what it says
//! ```
//!
//! A job is retired once it is Done ([`follow`]), which also ends a closing
//! kit with its last job.

use alloc::boxed::Box;
use core::mem;

use temper_lib::{Env, Id, Queue, ReplyTo, Slab, Time, Token};

use crate::boundary::{Done, Expect, Op, Request};
use crate::call::Outcome;
use crate::kit::{self, Kit};
use crate::knowledge::Knowledge;
use crate::limits::Limits;
use crate::model::Model;
use crate::path::Place;
use crate::window::{self, Span};

#[derive(Debug)]
pub(crate) struct Job {
    /// The kit whose call it runs.
    kit: Id<Kit>,
    state: State,
}

#[derive(Debug)]
enum State {
    /// Loading the file at `place`, to answer with a window of it.
    Reading { reply_to: ReplyTo, place: Place, span: Span },
    /// Scanning a directory.
    Listing { reply_to: ReplyTo },
    /// Storing the file at `place`, which it creates if `creating`, and
    /// replaces at the version the kit knows otherwise.
    Writing { reply_to: ReplyTo, place: Place, creating: bool },
    /// Terminal: answered, holds nothing.
    Done,
}

/// What a call that passed its kit's entrance does.
#[derive(Debug)]
pub(crate) enum Work {
    Read { place: Place, span: Span },
    List { place: Place },
    Write { place: Place, content: Box<[u8]>, expect: Expect },
}

/// Starts a job for `work` in the kit `kit`, which has room for one, asking
/// io for its operation by `deadline`, or sooner if the tools' own limit on
/// file operations falls first.
pub(crate) fn start(
    jobs: &mut Slab<Job>,
    kit: Id<Kit>,
    reply_to: ReplyTo,
    work: Work,
    deadline: Time,
    env: &Env<Limits>,
    out: &mut Queue<Request>,
) -> Id<Job> {
    let limits = &env.limits;
    let deadline = deadline.min(env.now.saturating_add(limits.file_timeout));
    // The place goes to io and stays with the job: copy at emission.
    let (state, op) = match work {
        Work::Read { place, span } => {
            let op = Op::Load { at: place.clone(), max: limits.file_bytes };
            (State::Reading { reply_to, place, span }, op)
        }
        Work::List { place } => (State::Listing { reply_to }, Op::Scan { at: place, max: limits.list_entries }),
        Work::Write { place, content, expect } => {
            let creating = match expect {
                Expect::Absent => true,
                Expect::Is { .. } => false,
            };
            let op = Op::Store { at: place.clone(), content, expect };
            (State::Writing { reply_to, place, creating }, op)
        }
    };
    // Room: a kit has at most `calls` jobs, and the slab twice that many slots
    // per kit, for the jobs retired in this iteration, which are at most those
    // running when it began.
    let id = jobs.insert(Job { kit, state }).expect("the job slab has room for every kit's jobs");
    out.push(Request::Io { owner: id.token(), op, deadline });
    id
}

/// Cancels the job `id`'s operation, for its kit is closing. The job answers
/// with whichever terminal comes.
pub(crate) fn cancel(id: Id<Job>, out: &mut Queue<Request>) {
    out.push(Request::CancelIo { owner: id.token() });
}

// The entry point for terminals: look the job up, take its state out, run the
// cell's handler, follow the new state.

pub(crate) fn done(model: &mut Model, env: &Env<Limits>, owner: Token, done: Done, out: &mut Queue<Request>) {
    let Model { kits, jobs } = model;
    let id = Id::from_token(owner);
    let job = jobs.get_mut(id).expect("a job lives until its operation has ended");
    let kit_id = job.kit;
    let kit = kits.get_mut(kit_id).expect("a kit lives until its jobs have ended");
    let state = mem::replace(&mut job.state, State::Done);
    job.state = match state {
        State::Reading { reply_to, place, span } => {
            loaded(&mut kit.knowledge, reply_to, place, span, done, &env.limits, out)
        }
        State::Listing { reply_to } => scanned(reply_to, done, out),
        State::Writing { reply_to, place, creating } => {
            stored(&mut kit.knowledge, reply_to, place, creating, done, out)
        }
        State::Done => unreachable!("a job that has answered has nothing in flight"),
    };
    follow(kits, jobs, id, out);
}

/// What a job's state implies, applied after every transition: a job that is
/// Done is retired, and leaves its kit.
fn follow(kits: &mut Slab<Kit>, jobs: &mut Slab<Job>, id: Id<Job>, out: &mut Queue<Request>) {
    let job = jobs.get(id).expect("a job lives until it is retired");
    match job.state {
        State::Reading { .. } | State::Listing { .. } | State::Writing { .. } => {}
        State::Done => {
            kit::finished(kits, job.kit, id, out);
            jobs.retire(id);
        }
    }
}

// Cell handlers: each takes the source state's data by value and returns the
// target state.

/// Reading, ended: answer with the window, and know the version read.
fn loaded(
    knowledge: &mut Knowledge,
    reply_to: ReplyTo,
    place: Place,
    span: Span,
    done: Done,
    limits: &Limits,
    out: &mut Queue<Request>,
) -> State {
    let outcome = match done {
        Done::Loaded { content, version } => {
            knowledge.record(place, version);
            window::window(content, span, limits.read_bytes)
        }
        Done::Missing => {
            knowledge.forget(&place);
            Outcome::NotFound
        }
        Done::NotFile => Outcome::NotFile,
        Done::NotDirectory => Outcome::NotDirectory,
        Done::TooLarge { size } => Outcome::TooLarge { size },
        Done::Escapes => Outcome::Outside,
        Done::Failed { fault } => Outcome::Failed { fault },
        Done::TimedOut => Outcome::TimedOut,
        Done::Cancelled => Outcome::Cancelled,
        Done::Scanned { .. } | Done::Stored { .. } | Done::Conflict { .. } => {
            unreachable!("io ends a load with a load's terminal")
        }
    };
    answer(reply_to, outcome, out)
}

/// Listing, ended: answer with the entries.
fn scanned(reply_to: ReplyTo, done: Done, out: &mut Queue<Request>) -> State {
    let outcome = match done {
        Done::Scanned { entries, more } => Outcome::Listed { entries, more },
        Done::Missing => Outcome::NotFound,
        Done::NotDirectory => Outcome::NotDirectory,
        Done::Escapes => Outcome::Outside,
        Done::Failed { fault } => Outcome::Failed { fault },
        Done::TimedOut => Outcome::TimedOut,
        Done::Cancelled => Outcome::Cancelled,
        Done::Loaded { .. } | Done::Stored { .. } | Done::Conflict { .. } | Done::NotFile | Done::TooLarge { .. } => {
            unreachable!("io ends a scan with a scan's terminal")
        }
    };
    answer(reply_to, outcome, out)
}

/// Writing, ended: answer, and know the version written.
fn stored(
    knowledge: &mut Knowledge,
    reply_to: ReplyTo,
    place: Place,
    creating: bool,
    done: Done,
    out: &mut Queue<Request>,
) -> State {
    let outcome = match done {
        Done::Stored { version } => {
            knowledge.record(place, version);
            Outcome::Written { created: creating }
        }
        // Something is there that the LLM has not read.
        Done::Conflict { now: Some(_) } if creating => Outcome::NotRead,
        Done::Conflict { now: None } if creating => unreachable!("io refuses to create a file only over one"),
        Done::Conflict { now: Some(_) } => Outcome::Stale,
        Done::Conflict { now: None } => {
            knowledge.forget(&place);
            Outcome::Stale
        }
        Done::NotFile => Outcome::NotFile,
        Done::NotDirectory => Outcome::NotDirectory,
        Done::Escapes => Outcome::Outside,
        Done::Failed { fault } => Outcome::Failed { fault },
        Done::TimedOut => Outcome::TimedOut,
        Done::Cancelled => Outcome::Cancelled,
        Done::Loaded { .. } | Done::Scanned { .. } | Done::Missing | Done::TooLarge { .. } => {
            unreachable!("io ends a store with a store's terminal")
        }
    };
    answer(reply_to, outcome, out)
}

fn answer(reply_to: ReplyTo, outcome: Outcome, out: &mut Queue<Request>) -> State {
    out.push(Request::Answer { to: reply_to, outcome });
    State::Done
}

/// The slots the job slab needs under `limits`, or `None` past a `u32`.
pub(crate) fn slots(limits: &Limits) -> Option<u32> {
    limits.kits.checked_mul(limits.calls)?.checked_mul(2)
}

/// What a running job holds beyond its slot: at most a place.
pub(crate) fn held(limits: &Limits) -> u64 {
    u64::from(limits.path_bytes)
}
