//! Fresh reads (engine-model.md, sections 4.4 and 12): what the parent reads
//! of the forge as it is now, before it plans a write, and for the runs and
//! briefs it serves. Each goes out ahead of everything else in the request
//! budget, is answered once, and leaves nothing behind: the working set is
//! kept by its own reads, not by these.
//!
//! A read that fails for a while (unavailable, a timeout) is tried again
//! after a backoff, up to `Limits::attempts`; a refusal for the rate waits
//! for its reset; any other failure is the answer.
//!
//! ```text
//! state    event                    next     requests
//! (none)   read, beyond the limits  (none)   read: invalid, busy
//!            or busy
//!          read                     Asking   (its call)
//! Asking   answered                 Closed   read
//!          failed, rate             Asking   (queued again)
//!          failed, may pass         Waiting  (or Closed, attempts spent: read)
//!          failed, otherwise        Closed   read: the error
//! Waiting  alarm                    Asking   (its call)
//! ```

use core::mem;

use temper_lib::bytes::copy_of;
use temper_lib::{Env, Id, Queue, Time, Token};

use crate::api::{Answer, Error, Op};
use crate::boundary::{Failure, Read, Request};
use crate::calls::{self, Purpose};
use crate::facts::Priority;
use crate::items;
use crate::limits::Limits;
use crate::model::{Alarm, Model};

/// A fresh read in hand.
#[derive(Debug)]
pub(crate) struct Fetch {
    owner: Token,
    read: Read,
    /// Attempts that failed and may pass.
    attempts: u32,
    state: State,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum State {
    /// Its call is queued or out.
    Asking,
    /// The last call failed: tried again at `until`.
    Waiting { until: Time },
    /// Terminal: holds nothing.
    Closed,
}

/// A fresh read from the parent: refused at the entrance, or asked for.
pub(crate) fn read(model: &mut Model, env: &Env<Limits>, owner: Token, read: Read, out: &mut Queue<Request>) {
    if !valid(&read, &env.limits) {
        out.push(Request::Read { owner, result: Err(Failure::Invalid) });
        return;
    }
    let fetch = Fetch { owner, read, attempts: 0, state: State::Asking };
    let Ok(id) = model.reads.insert(fetch) else {
        out.push(Request::Read { owner, result: Err(Failure::Busy) });
        return;
    };
    calls::queue(&mut model.calls, Purpose::Read(id), Priority::Fresh);
}

/// A backoff ended: the read is asked for again.
pub(crate) fn retry(model: &mut Model, env: &Env<Limits>, id: Id<Fetch>) {
    let fetch = model.reads.get_mut(id).expect("an alarm is cancelled as its read closes");
    match fetch.state {
        State::Waiting { until } => assert!(env.now >= until, "the backoff's alarm is armed for its end"),
        State::Asking | State::Closed => unreachable!("the backoff's alarm runs while it waits"),
    }
    fetch.state = State::Asking;
    calls::queue(&mut model.calls, Purpose::Read(id), Priority::Fresh);
}

/// Terminal for the read `id`'s call.
pub(crate) fn answered(
    model: &mut Model,
    env: &Env<Limits>,
    id: Id<Fetch>,
    result: Result<Answer, Error>,
    out: &mut Queue<Request>,
) {
    let fetch = model.reads.get_mut(id).expect("a read lives until its call is answered");
    match fetch.state {
        State::Asking => {}
        State::Waiting { .. } | State::Closed => unreachable!("a read's terminal comes while it asks"),
    }
    let error = match result {
        Ok(answer) => {
            answer_with(model, id, Ok(answer), out);
            return;
        }
        Err(error) => error,
    };
    match error {
        Error::RateLimited { .. } => {
            calls::queue(&mut model.calls, Purpose::Read(id), Priority::Fresh);
        }
        Error::Unavailable | Error::Timeout => {
            fetch.attempts = fetch.attempts.saturating_add(1);
            if fetch.attempts >= env.limits.attempts {
                answer_with(model, id, Err(Failure::Forge(error)), out);
                return;
            }
            let until = env.now.saturating_add(items::backoff(&env.limits, &mut model.rng, fetch.attempts));
            fetch.state = State::Waiting { until };
            model.alarms.arm(Alarm::Read(id), until).expect("an alarm per read fits");
        }
        Error::Forbidden
        | Error::Missing
        | Error::TooLarge
        | Error::Empty
        | Error::Full
        | Error::Exists
        | Error::NothingToMerge
        | Error::Closed
        | Error::Stale
        | Error::Conflict
        | Error::Protected => answer_with(model, id, Err(Failure::Forge(error)), out),
    }
}

/// Answers the read `id` and closes it.
fn answer_with(model: &mut Model, id: Id<Fetch>, result: Result<Answer, Failure>, out: &mut Queue<Request>) {
    let fetch = model.reads.get_mut(id).expect("a read lives until it is answered");
    let state = mem::replace(&mut fetch.state, State::Closed);
    match state {
        State::Asking => {}
        State::Waiting { .. } | State::Closed => unreachable!("a read is answered as it asks"),
    }
    out.push(Request::Read { owner: fetch.owner, result });
    model.alarms.cancel(Alarm::Read(id));
    model.reads.retire(id);
}

/// What the read `id`'s call asks, as it goes out.
pub(crate) fn op(model: &Model, id: Id<Fetch>) -> (u32, Op) {
    let fetch = model.reads.get(id).expect("a read lives until its call is answered");
    match &fetch.read {
        Read::Item { item, after } => (item.repository, Op::Item { number: item.number, after: *after }),
        Read::Pull { item } => (item.repository, Op::Pull { number: item.number }),
        Read::Reviews { item, page } => (item.repository, Op::Reviews { number: item.number, page: *page }),
        Read::PullFor { repository, head, base } => {
            (*repository, Op::PullFor { head: copy_of(head), base: copy_of(base) })
        }
        Read::Statuses { repository, commit, page } => (*repository, Op::Statuses { commit: *commit, page: *page }),
        Read::Permission { repository, user } => (*repository, Op::Permission { user: *user }),
        Read::Branch { repository, branch } => (*repository, Op::Branch { branch: copy_of(branch) }),
        Read::Pages { repository, after } => (*repository, Op::Pages { after: after.clone() }),
        Read::Page { repository, name } => (*repository, Op::Page { name: copy_of(name) }),
    }
}

/// Whether a read is within the limits, of the deployment's repositories.
fn valid(read: &Read, limits: &Limits) -> bool {
    let none: &[u8] = &[];
    let (repository, names): (u32, [&[u8]; 2]) = match read {
        Read::Item { item, .. } | Read::Pull { item } | Read::Reviews { item, .. } => (item.repository, [none, none]),
        Read::PullFor { repository, head, base } => (*repository, [head, base]),
        Read::Statuses { repository, .. } | Read::Permission { repository, .. } => (*repository, [none, none]),
        Read::Branch { repository, branch } => (*repository, [branch, none]),
        Read::Pages { repository, after } => match after {
            Some(after) => (*repository, [after, none]),
            None => (*repository, [none, none]),
        },
        Read::Page { repository, name } => (*repository, [name, none]),
    };
    if repository >= limits.repositories {
        return false;
    }
    for name in names {
        let fits = match u32::try_from(name.len()) {
            Ok(len) => len <= limits.name_bytes,
            Err(_) => false,
        };
        if !fits {
            return false;
        }
    }
    true
}
