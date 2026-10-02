//! Kits: one session's tools. A session opens its kit when it starts, with the
//! authority its opener gave it, and closes it when it ends.
//!
//! A call is checked at the entrance, where refusing it costs nothing: the
//! family of tools it belongs to must be granted, its path must lie in a
//! repository of the checkout, and in a writable one for a change, what it
//! would store must fit the limits, an edit must be of a file its LLM has read
//! and must change something, its deadline must not have passed, and the kit
//! must have room for one more job. A call that passes becomes a job.
//!
//! The transition table.
//!
//! ```text
//! state     event                  next      emits
//! Open      call                   Open      answer, or the job's operation
//!           close, no jobs         Closed    closed
//!           close, jobs            Closing   a cancel per job
//!           a job ends             Open
//! Closing   a job ends, not last   Closing
//!           the last job ends      Closed    closed
//! ```
//!
//! A call or a close to a kit that is not open is the session's bug. A kit is
//! retired once it is Closed.

use temper_lib::{Env, Id, Queue, ReplyTo, Set, Slab, Time, Token};

use crate::authority::{self, Authority, Checkout, Located};
use crate::boundary::{Expect, Refusal, Request};
use crate::call::{Call, Outcome};
use crate::edit::Edit;
use crate::job::{self, Job, Work};
use crate::knowledge::Knowledge;
use crate::limits::Limits;
use crate::model::Model;
use crate::path::Path;
use crate::window::Span;

#[derive(Debug)]
pub(crate) struct Kit {
    /// The session's token, echoed when the kit closes.
    session: Token,
    checkout: Checkout,
    pub(crate) knowledge: Knowledge,
    /// The jobs running its calls.
    jobs: Set<Id<Job>>,
    state: State,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum State {
    Open,
    /// Its jobs have been cancelled; it ends with the last.
    Closing,
    /// Terminal: retired, until the reclaim point frees it.
    Closed,
}

pub(crate) fn open(
    model: &mut Model,
    env: &Env<Limits>,
    session: Token,
    authority: Authority,
    out: &mut Queue<Request>,
) {
    if model.kits.is_full() {
        out.push(Request::Refused { session, refusal: Refusal::Busy });
        return;
    }
    let Some(checkout) = authority::admit(authority, &env.limits) else {
        out.push(Request::Refused { session, refusal: Refusal::Invalid });
        return;
    };
    let kit = Kit {
        session,
        checkout,
        knowledge: Knowledge::new(env.limits.known_files),
        jobs: Set::with_capacity(env.limits.calls),
        state: State::Open,
    };
    let id = model.kits.insert(kit).expect("checked for room above");
    out.push(Request::Opened { session, kit: id.token() });
}

pub(crate) fn call(
    model: &mut Model,
    env: &Env<Limits>,
    kit: Token,
    reply_to: ReplyTo,
    call: Call,
    deadline: Time,
    out: &mut Queue<Request>,
) {
    let Model { kits, jobs } = model;
    let id = Id::from_token(kit);
    let kit = kits.get_mut(id).expect("a kit lives until its session closes it");
    assert!(kit.state == State::Open, "no call follows a close");
    match admit(kit, call, deadline, env) {
        Ok(work) => {
            let job = job::start(jobs, id, reply_to, work, deadline, env, out);
            let fresh = kit.jobs.insert(job).expect("checked for room at the entrance");
            assert!(fresh, "a job is new to its kit");
        }
        Err(outcome) => out.push(Request::Answer { to: reply_to, outcome }),
    }
}

pub(crate) fn close(model: &mut Model, kit: Token, out: &mut Queue<Request>) {
    let id = Id::from_token(kit);
    let kit = model.kits.get_mut(id).expect("a kit lives until its session closes it");
    assert!(kit.state == State::Open, "a kit is closed once");
    if kit.jobs.is_empty() {
        end(&mut model.kits, id, out);
        return;
    }
    kit.state = State::Closing;
    for job in &kit.jobs {
        job::cancel(*job, out);
    }
}

/// Whether the kit is closing, its jobs cancelled.
pub(crate) fn closing(kit: &Kit) -> bool {
    match kit.state {
        State::Open => false,
        State::Closing => true,
        State::Closed => unreachable!("a closed kit runs no job"),
    }
}

/// The job `job` of the kit `id` has answered: it leaves the kit, which ends
/// with it if it was the last of a closing kit.
pub(crate) fn finished(kits: &mut Slab<Kit>, id: Id<Kit>, job: Id<Job>, out: &mut Queue<Request>) {
    let kit = kits.get_mut(id).expect("a kit lives until its jobs have ended");
    let running = kit.jobs.remove(&job);
    assert!(running, "a job is its kit's until it ends");
    match kit.state {
        State::Open => {}
        State::Closing => {
            if kit.jobs.is_empty() {
                end(kits, id, out);
            }
        }
        State::Closed => unreachable!("a closed kit runs no job"),
    }
}

/// Ends the kit `id`, which runs nothing: its session learns it has closed.
fn end(kits: &mut Slab<Kit>, id: Id<Kit>, out: &mut Queue<Request>) {
    let kit = kits.get_mut(id).expect("a kit lives until it is retired");
    kit.state = State::Closed;
    out.push(Request::Closed { session: kit.session });
    kits.retire(id);
}

/// What `call` does if it passes the entrance, or the outcome that refuses it.
fn admit(kit: &Kit, call: Call, deadline: Time, env: &Env<Limits>) -> Result<Work, Outcome> {
    let limits = &env.limits;
    if !authority::granted(kit.checkout.grants, &call) {
        return Err(Outcome::NotGranted);
    }
    let work = match call {
        Call::Read { path, skip, lines } => {
            let located = authority::locate(&kit.checkout, &path, limits.path_bytes)?;
            Work::Read { place: located.place, span: Span { skip, lines } }
        }
        Call::List { path } => {
            let located = authority::locate(&kit.checkout, &path, limits.path_bytes)?;
            Work::List { place: located.place }
        }
        Call::Search { path, .. } => {
            drop(authority::locate(&kit.checkout, &path, limits.path_bytes)?);
            return Err(Outcome::Unsupported);
        }
        Call::Write { path, content } => {
            let located = writable(&kit.checkout, &path, limits)?;
            fits(&content, limits)?;
            // As the kit knows the file: at the version its LLM read, or
            // absent.
            let expect = match kit.knowledge.version(&located.place) {
                Some(version) => Expect::Is { version },
                None => Expect::Absent,
            };
            Work::Write { place: located.place, content, expect }
        }
        Call::Edit { path, old, new, all } => {
            let located = writable(&kit.checkout, &path, limits)?;
            fits(&old, limits)?;
            fits(&new, limits)?;
            if old.is_empty() {
                return Err(Outcome::NoMatch);
            }
            if old == new {
                return Err(Outcome::Unchanged);
            }
            // A change needs the current version read: the load checks it is.
            if kit.knowledge.version(&located.place).is_none() {
                return Err(Outcome::NotRead);
            }
            Work::Edit { place: located.place, edit: Edit { old, new, all } }
        }
        Call::Shell { .. } => return Err(Outcome::Unsupported),
    };
    if deadline <= env.now {
        return Err(Outcome::TimedOut);
    }
    if kit.jobs.len() >= limits.calls {
        return Err(Outcome::Busy);
    }
    Ok(work)
}

/// Where `path` is, if the kit may write there.
fn writable(checkout: &Checkout, path: &Path, limits: &Limits) -> Result<Located, Outcome> {
    let located = authority::locate(checkout, path, limits.path_bytes)?;
    if !located.writable {
        return Err(Outcome::ReadOnly);
    }
    Ok(located)
}

/// Refuses `content` if it is larger than the tools store.
fn fits(content: &[u8], limits: &Limits) -> Result<(), Outcome> {
    let size = u64::try_from(content.len()).unwrap_or(u64::MAX);
    if size > u64::from(limits.file_bytes) {
        return Err(Outcome::TooLarge { size });
    }
    Ok(())
}
