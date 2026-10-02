//! Calls to the forge and the request budget (engine-model.md, section 12).
//!
//! Whatever needs the forge queues a call, in its priority's class: the
//! parent's fresh reads first, then writes and the reads that find what an
//! attempt made, then keeping up, then the slow pass. So that the first two
//! never starve the others, `Limits::reserve` calls of each window are kept
//! for keeping up and the slow pass while they wait. A call holds only what
//! it is for; its operation is built as it goes out, from its owner's state,
//! so a retry carries what is true then.
//!
//! Calls go out one per [`crate::resume`], while all of these hold: the
//! model has begun; one is queued; fewer than `Limits::calls` are out; the
//! budget's window is not spent; and no rate-limit refusal is waiting for its
//! reset. A model begins at its first moment, held as a refusal would hold
//! it until `Limits::lifetime` later, so that what an engine before it asked
//! for has landed before it reads the forge. The window
//! holds `Limits::rate` calls, starting with the first call after the last
//! window ended, as the forge's own does. A refusal for the rate stops every
//! call until the reset it names, and the refused call is queued again.
//!
//! A call's terminal comes back as [`crate::Event::Answered`], echoing the
//! token the call went out with: the call's own, which says what it was for.
//! A call is never withdrawn: an owner that has no more use for it waits for
//! its terminal before it is retired, so there is never more than one call
//! per owner, and the queues never hold more calls than there are owners.

use temper_lib::{Env, Id, Queue, Slab, Time, Token};

use crate::api::{Answer, Error, Op};
use crate::boundary::Request;
use crate::facts::{Fact, Priority};
use crate::items::{self, Entry};
use crate::limits::Limits;
use crate::model::{Alarm, Model};
use crate::reads::{self, Fetch};
use crate::scans;
use crate::writes::{self, Writing};

#[derive(Debug)]
pub(crate) struct Call {
    purpose: Purpose,
    priority: Priority,
    state: State,
}

/// What a call is for: its owner.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Purpose {
    Read(Id<Fetch>),
    Write(Id<Writing>),
    Item(Id<Entry>),
    /// A repository's listing.
    Listing(u32),
    /// A repository's slow pass: a listing or a probe.
    Slow(u32),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum State {
    /// Waiting in its class's queue.
    Queued,
    /// Out: its terminal is due.
    Out,
    /// Terminal: answered.
    Closed,
}

/// The calls in hand, the queues of each class, and the budget.
#[derive(Debug)]
pub(crate) struct Calls {
    slab: Slab<Call>,
    fresh: Queue<Id<Call>>,
    write: Queue<Id<Call>>,
    keep: Queue<Id<Call>>,
    slow: Queue<Id<Call>>,
    /// Calls out, and how many may be.
    out: u32,
    limit: u32,
    budget: Budget,
    /// Whether the model's first moment has come.
    woken: bool,
}

/// The request budget: calls left in the window and when it ends, the
/// parent's reads and the writes it took, whether it is spent, and the reset
/// a rate-limit refusal named, until it passes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Budget {
    left: u32,
    ends: Time,
    first: u32,
    spent: bool,
    reset: Option<Time>,
}

impl Calls {
    /// Room for `capacity` calls in hand, `limit` of them out at once.
    pub(crate) fn with_capacity(capacity: u32, limit: u32) -> Calls {
        Calls {
            slab: Slab::with_capacity(capacity),
            fresh: Queue::with_capacity(capacity),
            write: Queue::with_capacity(capacity),
            keep: Queue::with_capacity(capacity),
            slow: Queue::with_capacity(capacity),
            out: 0,
            limit,
            budget: Budget { left: 0, ends: Time::ZERO, first: 0, spent: false, reset: None },
            woken: false,
        }
    }

    pub(crate) fn held(&self) -> u32 {
        self.slab.len()
    }

    pub(crate) fn out(&self) -> u32 {
        self.out
    }

    /// Whether a call may go out: see the module doc.
    pub(crate) fn is_ready(&self) -> bool {
        let queued = !(self.fresh.is_empty() && self.write.is_empty() && self.keep.is_empty() && self.slow.is_empty());
        self.woken && queued && !self.budget.spent && self.budget.reset.is_none() && self.out < self.limit
    }

    pub(crate) fn reclaim(&mut self) {
        self.slab.reclaim();
    }
}

/// Queues a call for `purpose` in the class `priority`, and returns it.
pub(crate) fn queue(calls: &mut Calls, purpose: Purpose, priority: Priority) -> Id<Call> {
    let call = Call { purpose, priority, state: State::Queued };
    let id = calls.slab.insert(call).expect("a call per owner fits");
    let queue = match priority {
        Priority::Fresh => &mut calls.fresh,
        Priority::Write => &mut calls.write,
        Priority::Keep => &mut calls.keep,
        Priority::Slow => &mut calls.slow,
    };
    queue.push(id);
    id
}

/// Sends the next call queued, the highest class first, if the budget allows:
/// at most one request.
pub(crate) fn send(model: &mut Model, env: &Env<Limits>, out: &mut Queue<Request>) {
    if !model.calls.is_ready() {
        return;
    }
    let Some(id) = next(&mut model.calls, env) else {
        return;
    };
    let call = model.calls.slab.get_mut(id).expect("a call queued lives until it is answered");
    match call.state {
        State::Queued => {}
        State::Out | State::Closed => unreachable!("a call is queued once, and goes out from the queue"),
    }
    call.state = State::Out;
    let (purpose, priority) = (call.purpose, call.priority);
    model.calls.out = model.calls.out.saturating_add(1);
    spend(model, env, priority);
    let (repository, op) = build(model, env, purpose);
    match purpose {
        Purpose::Write(owner) => writes::sent(model, owner, env.now),
        Purpose::Read(_) | Purpose::Item(_) | Purpose::Listing(_) | Purpose::Slow(_) => {}
    }
    model.facts.push(Fact::Sent { priority });
    out.push(Request::Call { call: id.token(), repository, op });
}

/// The next call in the queues, the highest class first; but the first two
/// classes only within their share of the window while the others wait.
fn next(calls: &mut Calls, env: &Env<Limits>) -> Option<Id<Call>> {
    let waiting = !(calls.keep.is_empty() && calls.slow.is_empty());
    let first = if env.now >= calls.budget.ends { 0 } else { calls.budget.first };
    let share = env.limits.rate.saturating_sub(env.limits.reserve);
    if !waiting || first < share {
        if let Some(id) = calls.fresh.pop() {
            return Some(id);
        }
        if let Some(id) = calls.write.pop() {
            return Some(id);
        }
    }
    if let Some(id) = calls.keep.pop() {
        return Some(id);
    }
    calls.slow.pop()
}

/// Spends one call of the window, of the class `priority`, starting a new
/// window if the last ended.
fn spend(model: &mut Model, env: &Env<Limits>, priority: Priority) {
    let budget = &mut model.calls.budget;
    if env.now >= budget.ends {
        budget.left = env.limits.rate;
        budget.ends = env.now.saturating_add(env.limits.window);
        budget.first = 0;
    }
    match priority {
        Priority::Fresh | Priority::Write => budget.first = budget.first.saturating_add(1),
        Priority::Keep | Priority::Slow => {}
    }
    budget.left = budget.left.saturating_sub(1);
    if budget.left == 0 {
        budget.spent = true;
        let until = budget.ends;
        model.alarms.arm(Alarm::Window, until).expect("the budget's alarms fit");
        model.facts.push(Fact::Spent { until });
    }
}

/// What a call asks, from its owner's state as it goes out.
fn build(model: &Model, env: &Env<Limits>, purpose: Purpose) -> (u32, Op) {
    match purpose {
        Purpose::Read(id) => reads::op(model, id),
        Purpose::Write(id) => writes::op(model, env, id),
        Purpose::Item(id) => items::op(model, id),
        Purpose::Listing(repository) => scans::op(model, repository),
        Purpose::Slow(repository) => scans::slow_op(model, repository),
    }
}

/// The model's first moment: it begins, and holds its calls until nothing a
/// model before it asked for can still land.
pub(crate) fn wake(model: &mut Model, env: &Env<Limits>) {
    model.calls.woken = true;
    let until = env.now.saturating_add(env.limits.lifetime);
    if until > env.now {
        model.calls.budget.reset = Some(until);
        model.alarms.arm(Alarm::Reset, until).expect("the budget's alarms fit");
    }
}

/// The window's alarm: the budget is whole again.
pub(crate) fn window(model: &mut Model, env: &Env<Limits>) {
    let budget = &mut model.calls.budget;
    assert!(env.now >= budget.ends, "the window's alarm is armed for its end");
    budget.spent = false;
}

/// The reset's alarm: the forge takes calls again.
pub(crate) fn reset(model: &mut Model, env: &Env<Limits>) {
    let budget = &mut model.calls.budget;
    match budget.reset {
        Some(at) => assert!(env.now >= at, "the reset's alarm is armed for it"),
        None => unreachable!("the reset's alarm runs while a reset is awaited"),
    }
    budget.reset = None;
}

/// Terminal for a call: its owner hears how it went. A refusal for the rate
/// holds every call until its reset.
pub(crate) fn answered(
    model: &mut Model,
    env: &Env<Limits>,
    token: Token,
    result: Result<Answer, Error>,
    out: &mut Queue<Request>,
) {
    let id = Id::<Call>::from_token(token);
    let call = model.calls.slab.get_mut(id).expect("a call's terminal comes while it is out");
    match call.state {
        State::Out => {}
        State::Queued | State::Closed => unreachable!("one terminal per call, once it is out"),
    }
    call.state = State::Closed;
    let (purpose, priority) = (call.purpose, call.priority);
    model.calls.slab.retire(id);
    model.calls.out = model.calls.out.saturating_sub(1);
    if let Err(error) = &result {
        model.facts.push(Fact::Failed { priority, error: *error });
        match error {
            Error::RateLimited { after } => limited(model, env, env.now.saturating_add(*after)),
            Error::Unavailable
            | Error::Timeout
            | Error::Forbidden
            | Error::Missing
            | Error::TooLarge
            | Error::Empty
            | Error::Full
            | Error::Exists
            | Error::NothingToMerge
            | Error::Closed
            | Error::Stale
            | Error::Conflict
            | Error::Protected => {}
        }
    }
    match purpose {
        Purpose::Read(owner) => reads::answered(model, env, owner, result, out),
        Purpose::Write(owner) => writes::answered(model, env, owner, result, out),
        Purpose::Item(owner) => items::answered(model, env, owner, result, out),
        Purpose::Listing(repository) => scans::answered(model, env, repository, result, out),
        Purpose::Slow(repository) => scans::slow_answered(model, env, repository, result),
    }
}

/// A refusal for the rate: nothing goes out until `reset`.
fn limited(model: &mut Model, env: &Env<Limits>, reset: Time) {
    model.facts.push(Fact::Limited { reset });
    if reset <= env.now {
        return;
    }
    let budget = &mut model.calls.budget;
    let until = match budget.reset {
        Some(at) if at >= reset => at,
        Some(_) | None => reset,
    };
    budget.reset = Some(until);
    model.alarms.arm(Alarm::Reset, until).expect("the budget's alarms fit");
}
