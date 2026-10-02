//! Jobs: the calls a kit is running, each waiting on one operation of io.
//!
//! A call that passes the kit's entrance becomes a job, which asks io for its
//! operation and answers the call once that operation has ended. Every
//! operation ends in exactly one terminal event, so a job answers exactly once,
//! whether the operation completed, failed, timed out, or lost or won a race
//! with a cancel.
//!
//! A write stores the file if it is as the kit knows it: at the version its
//! LLM read or wrote last, or absent if it knows none. An edit runs in two
//! phases: it loads the file, which must be at the version the kit knows,
//! makes the edit in memory, and stores the result expecting that version.
//! io compares that with the real file just before it stores, so a change
//! made since, by another kit or by anything else, is caught: as `Stale` if
//! the LLM had read the file, and as `NotRead` if it had not and the write
//! would have created it. A store follows no symbolic link, so a change
//! lands only in the repository its path names. A write that
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
//! Editing    loaded, as known, edited    Storing   (store the edited file)
//!            loaded, otherwise           Done      stale, no match, ambiguous, too large,
//!                                                  or cancelled if the kit is closing
//!            missing                     Done      not found; nothing is known there
//!            any other                   Done      what it says
//! Running    exited                      Done      how it ended, and its output
//!            any other                   Done      what it says
//! Searching  found                       Done      the lines found
//!            exited                      Done      how rg failed
//!            any other                   Done      what it says
//! Storing    stored                      Done      written or edited; the new version is known
//!            conflict, creating          Done      not read
//!            conflict, replacing         Done      stale; a file now absent is forgotten
//!            any other                   Done      what it says
//! ```
//!
//! A command runs until it exits or its deadline, the call's or the tools'
//! own limit for commands, whichever is sooner. It may change any file the kit
//! may write; what the kit knows stays as it was, and the version check of
//! the next write or edit catches what the command changed.
//!
//! A kit that closes while an edit loads stores nothing: whichever way the
//! load's race with its cancel went, the edit answers `Cancelled`.
//!
//! A job is retired once it is Done ([`follow`]), which also ends a closing
//! kit with its last job.

use alloc::boxed::Box;
use core::mem;

use temper_lib::{Duration, Env, Id, Queue, ReplyTo, Slab, Time, Token};

use crate::authority::Var;
use crate::boundary::{Done, Expect, Op, Request, Root};
use crate::call::Outcome;
use crate::edit::{self, Edit};
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
    /// Loading the file to edit.
    Editing { reply_to: ReplyTo, editing: Editing },
    /// Storing the file at `place`, for `change`.
    Storing { reply_to: ReplyTo, place: Place, change: Change },
    /// Running a command.
    Running { reply_to: ReplyTo },
    /// Searching files.
    Searching { reply_to: ReplyTo },
    /// Terminal: answered, holds nothing.
    Done,
}

/// An edit while it loads the file.
#[derive(Debug)]
pub(crate) struct Editing {
    place: Place,
    edit: Edit,
    /// The call's, which its store keeps too.
    deadline: Time,
}

/// What a store does to the file, and so what it answers.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Change {
    /// Writes a file the kit knows nothing of.
    Create,
    /// Writes over the file at the version the kit knows.
    Replace,
    /// Writes the file edited, having `replaced` that many occurrences.
    Edit { replaced: u32 },
}

/// What a call that passed its kit's entrance does.
#[derive(Debug)]
pub(crate) enum Work {
    Read {
        place: Place,
        span: Span,
    },
    List {
        place: Place,
    },
    Write {
        place: Place,
        content: Box<[u8]>,
        expect: Expect,
    },
    Edit {
        place: Place,
        edit: Edit,
    },
    /// Runs `command` in `cwd` for at most `timeout`, with the kit's `env`,
    /// seeing its `roots`: copies of the kit's, made for the request.
    Shell {
        cwd: Place,
        command: Box<[u8]>,
        timeout: Duration,
        env: Box<[Var]>,
        roots: Box<[Root]>,
    },
    Search {
        place: Place,
        pattern: Box<[u8]>,
        glob: Option<Box<[u8]>>,
    },
}

/// Starts a job for `work` in the kit `kit`, which has room for one, asking
/// io for its operation by `deadline`, or sooner if the tools' own limit on
/// file operations, or on commands, falls first.
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
    let call_deadline = deadline;
    // A command has a limit of its own; any other call is a file operation.
    let limit = match &work {
        Work::Shell { timeout, .. } => *timeout,
        Work::Search { .. } => limits.search_timeout,
        Work::Read { .. } | Work::List { .. } | Work::Write { .. } | Work::Edit { .. } => limits.file_timeout,
    };
    let deadline = deadline.min(env.now.saturating_add(limit));
    // The place goes to io and stays with the job: copy at emission.
    let (state, op) = match work {
        Work::Read { place, span } => {
            let op = Op::Load { at: place.clone(), max: limits.file_bytes };
            (State::Reading { reply_to, place, span }, op)
        }
        Work::List { place } => (State::Listing { reply_to }, Op::Scan { at: place, max: limits.list_entries }),
        Work::Write { place, content, expect } => {
            let change = match expect {
                Expect::Absent => Change::Create,
                Expect::Is { .. } => Change::Replace,
            };
            let op = Op::Store { at: place.clone(), content, expect };
            (State::Storing { reply_to, place, change }, op)
        }
        Work::Edit { place, edit } => {
            let op = Op::Load { at: place.clone(), max: limits.file_bytes };
            (State::Editing { reply_to, editing: Editing { place, edit, deadline: call_deadline } }, op)
        }
        Work::Shell { cwd, command, timeout: _, env, roots } => {
            let op = Op::Spawn { cwd, command, env, roots, head: limits.shell_head, tail: limits.shell_tail };
            (State::Running { reply_to }, op)
        }
        Work::Search { place, pattern, glob } => {
            let op = Op::Search { at: place, pattern, glob, hits: limits.search_hits, bytes: limits.search_bytes };
            (State::Searching { reply_to }, op)
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
        State::Editing { reply_to, editing } => to_edit(kit, id, reply_to, editing, done, env, out),
        State::Storing { reply_to, place, change } => stored(&mut kit.knowledge, reply_to, place, change, done, out),
        State::Running { reply_to } => exited(reply_to, done, out),
        State::Searching { reply_to } => found(reply_to, done, out),
        State::Done => unreachable!("a job that has answered has nothing in flight"),
    };
    follow(kits, jobs, id, out);
}

/// What a job's state implies, applied after every transition: a job that is
/// Done is retired, and leaves its kit.
fn follow(kits: &mut Slab<Kit>, jobs: &mut Slab<Job>, id: Id<Job>, out: &mut Queue<Request>) {
    let job = jobs.get(id).expect("a job lives until it is retired");
    match job.state {
        State::Reading { .. }
        | State::Listing { .. }
        | State::Editing { .. }
        | State::Storing { .. }
        | State::Running { .. }
        | State::Searching { .. } => {}
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
        Done::Scanned { .. }
        | Done::Stored { .. }
        | Done::Conflict { .. }
        | Done::Linked
        | Done::Exited { .. }
        | Done::Found { .. } => {
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
        Done::Loaded { .. }
        | Done::Stored { .. }
        | Done::Conflict { .. }
        | Done::NotFile
        | Done::Linked
        | Done::TooLarge { .. }
        | Done::Exited { .. }
        | Done::Found { .. } => unreachable!("io ends a scan with a scan's terminal"),
    };
    answer(reply_to, outcome, out)
}

/// Editing, loaded: make the edit and store it, if the file is as its LLM
/// read it and the kit is not closing.
fn to_edit(
    kit: &mut Kit,
    id: Id<Job>,
    reply_to: ReplyTo,
    editing: Editing,
    done: Done,
    env: &Env<Limits>,
    out: &mut Queue<Request>,
) -> State {
    let Editing { place, edit, deadline } = editing;
    let outcome = match done {
        Done::Loaded { content, version } => {
            if kit::closing(kit) {
                return answer(reply_to, Outcome::Cancelled, out);
            }
            if kit.knowledge.version(&place) != Some(version) {
                return answer(reply_to, Outcome::Stale, out);
            }
            if deadline <= env.now {
                return answer(reply_to, Outcome::TimedOut, out);
            }
            match edit::apply(&content, &edit, &env.limits) {
                Ok((content, replaced)) => {
                    let deadline = deadline.min(env.now.saturating_add(env.limits.file_timeout));
                    let op = Op::Store { at: place.clone(), content, expect: Expect::Is { version } };
                    out.push(Request::Io { owner: id.token(), op, deadline });
                    return State::Storing { reply_to, place, change: Change::Edit { replaced } };
                }
                Err(outcome) => outcome,
            }
        }
        Done::Missing => {
            kit.knowledge.forget(&place);
            Outcome::NotFound
        }
        Done::NotFile => Outcome::NotFile,
        Done::NotDirectory => Outcome::NotDirectory,
        Done::TooLarge { size } => Outcome::TooLarge { size },
        Done::Escapes => Outcome::Outside,
        Done::Failed { fault } => Outcome::Failed { fault },
        Done::TimedOut => Outcome::TimedOut,
        Done::Cancelled => Outcome::Cancelled,
        Done::Scanned { .. }
        | Done::Stored { .. }
        | Done::Conflict { .. }
        | Done::Linked
        | Done::Exited { .. }
        | Done::Found { .. } => {
            unreachable!("io ends a load with a load's terminal")
        }
    };
    answer(reply_to, outcome, out)
}

/// Storing, ended: answer, and know the version written.
fn stored(
    knowledge: &mut Knowledge,
    reply_to: ReplyTo,
    place: Place,
    change: Change,
    done: Done,
    out: &mut Queue<Request>,
) -> State {
    let creating = change == Change::Create;
    let outcome = match done {
        Done::Stored { version } => {
            knowledge.record(place, version);
            match change {
                Change::Create => Outcome::Written { created: true },
                Change::Replace => Outcome::Written { created: false },
                Change::Edit { replaced } => Outcome::Edited { replaced },
            }
        }
        // Something was there that the LLM has not read, even if it is gone
        // by the time io looked for its version.
        Done::Conflict { .. } if creating => Outcome::NotRead,
        Done::Conflict { now: Some(_) } => Outcome::Stale,
        Done::Conflict { now: None } => {
            knowledge.forget(&place);
            Outcome::Stale
        }
        Done::NotFile => Outcome::NotFile,
        Done::Linked => Outcome::Linked,
        Done::NotDirectory => Outcome::NotDirectory,
        Done::Escapes => Outcome::Outside,
        Done::Failed { fault } => Outcome::Failed { fault },
        Done::TimedOut => Outcome::TimedOut,
        Done::Cancelled => Outcome::Cancelled,
        Done::Loaded { .. }
        | Done::Scanned { .. }
        | Done::Missing
        | Done::TooLarge { .. }
        | Done::Exited { .. }
        | Done::Found { .. } => {
            unreachable!("io ends a store with a store's terminal")
        }
    };
    answer(reply_to, outcome, out)
}

/// Searching, ended: answer with the lines found, or how rg failed.
fn found(reply_to: ReplyTo, done: Done, out: &mut Queue<Request>) -> State {
    let outcome = match done {
        Done::Found { hits, more } => Outcome::Found { hits, more },
        Done::Exited { exit, head, tail, dropped } => Outcome::Exited { exit, head, tail, dropped },
        Done::Missing => Outcome::NotFound,
        Done::NotDirectory => Outcome::NotDirectory,
        Done::Escapes => Outcome::Outside,
        Done::Failed { fault } => Outcome::Failed { fault },
        Done::TimedOut => Outcome::TimedOut,
        Done::Cancelled => Outcome::Cancelled,
        Done::Loaded { .. }
        | Done::Scanned { .. }
        | Done::Stored { .. }
        | Done::Conflict { .. }
        | Done::NotFile
        | Done::Linked
        | Done::TooLarge { .. } => unreachable!("io ends a search with a search's terminal"),
    };
    answer(reply_to, outcome, out)
}

/// Running, ended: answer with how the command ended and what it wrote.
fn exited(reply_to: ReplyTo, done: Done, out: &mut Queue<Request>) -> State {
    let outcome = match done {
        Done::Exited { exit, head, tail, dropped } => Outcome::Exited { exit, head, tail, dropped },
        // The working directory.
        Done::Missing => Outcome::NotFound,
        Done::NotDirectory => Outcome::NotDirectory,
        Done::Escapes => Outcome::Outside,
        Done::Failed { fault } => Outcome::Failed { fault },
        Done::TimedOut => Outcome::TimedOut,
        Done::Cancelled => Outcome::Cancelled,
        Done::Loaded { .. }
        | Done::Scanned { .. }
        | Done::Stored { .. }
        | Done::Conflict { .. }
        | Done::NotFile
        | Done::Linked
        | Done::TooLarge { .. }
        | Done::Found { .. } => unreachable!("io ends a spawn with a spawn's terminal"),
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

/// What a running job holds beyond its slot, or `None` past a `u64`: at most
/// a place, and an edit's snippets.
pub(crate) fn held(limits: &Limits) -> Option<u64> {
    u64::from(limits.file_bytes).checked_mul(2)?.checked_add(u64::from(limits.path_bytes))
}
