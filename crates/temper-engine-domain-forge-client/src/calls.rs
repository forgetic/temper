//! Request accounting and retry scheduling (domain/forge.md, section 5.4).
//!
//! Calls retain four priority queues, in-flight tokens and rate windows;
//! they know neither tasks nor provider history. `queue` admits a logical
//! call, `send` emits one request, and `answered` settles its actual cost.
//! Queued → Out → Closed on success or permanent failure; a transient
//! failure moves Out → Waiting → Queued. A rate refusal holds every class
//! until reset without completing the logical read twice.
use crate::api::{Answer, Error, Op, Repository};
use crate::domain::{Alarm, Domain};
use crate::{Fact, Limits, Priority, Request, bounds};
use skein_lib::{Env, Id, Map, Queue, Slab, Time, Token};

#[derive(Debug)]
pub(crate) struct Call {
    owner: Owner,
    repository: Repository,
    op: Op,
    priority: Priority,
    state: State,
    window: Time,
    failures: u32,
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Owner {
    Read(Token),
    Entry(u64),
    Resource(crate::Resource),
    Repository(Repository),
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum State {
    Queued,
    Out,
    Waiting,
    Closed,
}
#[derive(Debug)]
pub(crate) struct Calls {
    slab: Slab<Call>,
    flights: Map<Token, Id<Call>>,
    sequence: u64,
    fresh: Queue<Id<Call>>,
    write: Queue<Id<Call>>,
    keep: Queue<Id<Call>>,
    slow: Queue<Id<Call>>,
    budget: Budget,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Budget {
    left: u32,
    ends: Time,
    first: u32,
    spent: bool,
    reset: Option<Time>,
}
impl Calls {
    pub(crate) fn new(l: &Limits) -> Calls {
        Calls {
            slab: Slab::with_capacity(l.pending),
            flights: Map::with_capacity(l.calls),
            sequence: 0,
            fresh: Queue::with_capacity(l.pending),
            write: Queue::with_capacity(l.pending),
            keep: Queue::with_capacity(l.pending),
            slow: Queue::with_capacity(l.pending),
            budget: Budget { left: 0, ends: Time::ZERO, first: 0, spent: false, reset: None },
        }
    }
    pub(crate) fn is_full(&self) -> bool {
        self.slab.is_full()
    }
    pub(crate) fn is_ready(&self) -> bool {
        let queued = !(self.fresh.is_empty() && self.write.is_empty() && self.keep.is_empty() && self.slow.is_empty());
        queued && !self.budget.spent && self.budget.reset.is_none() && self.flights.len() < self.flights.capacity()
    }
    pub(crate) fn reclaim(&mut self) {
        self.slab.reclaim();
    }
    #[cfg(test)]
    pub(crate) const fn remaining(&self) -> (u32, u32) {
        (self.budget.left, self.budget.first)
    }
}
/// Caller has checked operation size and admission before entering.
pub(crate) fn queue(c: &mut Calls, owner: Owner, repository: Repository, op: Op, priority: Priority) {
    let call = Call { owner, repository, op, priority, state: State::Queued, window: Time::ZERO, failures: 0 };
    let id = c.slab.insert(call).expect("one admitted call fits");
    queued(c, id, priority);
}
fn queued(c: &mut Calls, id: Id<Call>, priority: Priority) {
    let queue = match priority {
        Priority::Fresh => &mut c.fresh,
        Priority::Write => &mut c.write,
        Priority::Keep => &mut c.keep,
        Priority::Slow => &mut c.slow,
    };
    queue.push(id);
}
/// Remove a call which has not been emitted. An outstanding peer request is
/// never cancelled here: its one terminal still belongs to its owner.
pub(crate) fn cancel_entry(c: &mut Calls, number: u64) -> bool {
    cancel(c, &Owner::Entry(number))
}
pub(crate) fn cancel_resource(c: &mut Calls, resource: &crate::Resource) -> bool {
    cancel(c, &Owner::Resource(resource.clone()))
}
pub(crate) fn cancel_repository(c: &mut Calls, repository: Repository) -> bool {
    cancel(c, &Owner::Repository(repository))
}
fn cancel(c: &mut Calls, owner: &Owner) -> bool {
    let mut cancelled = false;
    for priority in [Priority::Fresh, Priority::Write, Priority::Keep, Priority::Slow] {
        let queue = match priority {
            Priority::Fresh => &mut c.fresh,
            Priority::Write => &mut c.write,
            Priority::Keep => &mut c.keep,
            Priority::Slow => &mut c.slow,
        };
        for _ in 0..queue.len() {
            let id = queue.pop().expect("bounded queue length measured");
            let call = c.slab.get_mut(id).expect("a queued call remains");
            if call.owner == *owner {
                match call.state {
                    State::Queued => call.state = State::Closed,
                    State::Out | State::Waiting | State::Closed => {
                        unreachable!("only queued calls sit in a class queue")
                    }
                }
                c.slab.retire(id);
                cancelled = true;
            } else {
                queue.push(id);
            }
        }
    }
    cancelled
}
pub(crate) fn send(d: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    if !d.calls.is_ready() {
        return;
    }
    let Some(id) = next(&mut d.calls, env) else {
        return;
    };
    let call = d.calls.slab.get_mut(id).expect("a queued call stays alive");
    match call.state {
        State::Queued => call.state = State::Out,
        State::Out | State::Waiting | State::Closed => unreachable!("a call is queued exactly once"),
    }
    let priority = call.priority;
    let owner = call.owner.clone();
    match owner {
        Owner::Entry(number) => crate::outbox::sent(d, env, number, out),
        Owner::Read(_) | Owner::Resource(_) | Owner::Repository(_) => {}
    }
    spend(d, env, priority);
    let call = d.calls.slab.get_mut(id).expect("the sent call remains");
    call.window = d.calls.budget.ends;
    let repository = call.repository;
    let op = call.op.clone();
    d.calls.sequence = d.calls.sequence.checked_add(1).expect("call sequence does not exhaust in a process");
    let token = Token::new(d.calls.sequence);
    let inserted = d.calls.flights.insert(token, id).expect("in-flight admission checked");
    assert!(inserted.is_none(), "call tokens are distinct, including retries");
    d.fact(Fact::Sent { priority });
    out.push(Request::Call { call: token, repository, op });
}
fn next(c: &mut Calls, env: &Env<Limits>) -> Option<Id<Call>> {
    let waiting = !(c.keep.is_empty() && c.slow.is_empty());
    let first = if env.now >= c.budget.ends { 0 } else { c.budget.first };
    let share = env.limits.rate.saturating_sub(env.limits.reserve);
    if !waiting || first < share {
        if let Some(id) = c.fresh.pop() {
            return Some(id);
        }
        if let Some(id) = c.write.pop() {
            return Some(id);
        }
    }
    if let Some(id) = c.keep.pop() {
        return Some(id);
    }
    c.slow.pop()
}
/// Apply the same reserved share before allocating a logical-call slot:
/// otherwise a sole slot can repeatedly be filled by foreground pumps before
/// a due background operation ever becomes visible to `next`.
pub(crate) fn background_owed(c: &Calls, env: &Env<Limits>) -> bool {
    let first = if env.now >= c.budget.ends { 0 } else { c.budget.first };
    env.limits.reserve > 0 && first >= env.limits.rate.saturating_sub(env.limits.reserve)
}
fn spend(d: &mut Domain, env: &Env<Limits>, priority: Priority) {
    let b = &mut d.calls.budget;
    if env.now >= b.ends {
        b.left = env.limits.rate;
        b.ends = env.now.saturating_add(env.limits.window);
        b.first = 0;
    }
    match priority {
        Priority::Fresh | Priority::Write => b.first = b.first.saturating_add(1),
        Priority::Keep | Priority::Slow => {}
    }
    b.left = b.left.saturating_sub(1);
    if b.left == 0 {
        b.spent = true;
        let until = b.ends;
        d.alarms.arm(Alarm::Window, until).expect("two budget alarms fit");
        d.fact(Fact::Spent { until });
    }
}
fn settle(d: &mut Domain, env: &Env<Limits>, priority: Priority, window: Time, cost: u32) {
    let b = &mut d.calls.budget;
    if cost == 0 {
        if window == b.ends && env.now < b.ends {
            b.left = b.left.saturating_add(1).min(env.limits.rate);
            match priority {
                Priority::Fresh | Priority::Write => b.first = b.first.saturating_sub(1),
                Priority::Keep | Priority::Slow => {}
            }
            if b.spent && b.left > 0 {
                b.spent = false;
                d.alarms.cancel(Alarm::Window);
            }
        }
        return;
    }
    let extra = cost.saturating_sub(1);
    if extra == 0 {
        return;
    }
    if env.now >= b.ends {
        b.left = env.limits.rate;
        b.ends = env.now.saturating_add(env.limits.window);
        b.first = 0;
        b.spent = false;
        d.alarms.cancel(Alarm::Window);
    }
    match priority {
        Priority::Fresh | Priority::Write => b.first = b.first.saturating_add(extra),
        Priority::Keep | Priority::Slow => {}
    }
    b.left = b.left.saturating_sub(extra);
    if b.left == 0 && !b.spent {
        b.spent = true;
        let until = b.ends;
        d.alarms.arm(Alarm::Window, until).expect("two budget alarms fit");
        d.fact(Fact::Spent { until });
    }
}
pub(crate) fn window(d: &mut Domain, env: &Env<Limits>) {
    assert!(env.now >= d.calls.budget.ends, "window expires at its end");
    d.calls.budget.spent = false;
}
pub(crate) fn reset(d: &mut Domain, env: &Env<Limits>) {
    let at = d.calls.budget.reset.expect("reset is armed while refused");
    assert!(env.now >= at, "reset expires at its deadline");
    d.calls.budget.reset = None;
}
pub(crate) fn answered(
    d: &mut Domain,
    env: &Env<Limits>,
    token: Token,
    cost: u32,
    result: Result<Answer, Error>,
    out: &mut Queue<Request>,
) {
    let id = d.calls.flights.remove(&token).expect("exactly one terminal per call token");
    let call = d.calls.slab.get_mut(id).expect("an in-flight call remains");
    match call.state {
        State::Out => call.state = State::Closed,
        State::Queued | State::Waiting | State::Closed => unreachable!("terminal belongs to an outstanding call"),
    }
    let owner = call.owner.clone();
    let priority = call.priority;
    let window = call.window;
    settle(d, env, priority, window, cost);
    let result = match result {
        Ok(answer) => {
            let op = &d.calls.slab.get(id).expect("the terminal call remains").op;
            if !bounds::answer(&answer, &env.limits) {
                match op {
                    Op::Read(_) => Err(Error::TooLarge),
                    Op::Write(_) => Err(Error::InvalidAnswer),
                }
            } else if !crate::api::accepts(op, &answer) {
                Err(Error::InvalidAnswer)
            } else {
                Ok(answer)
            }
        }
        Err(error) => Err(error),
    };
    if let Err(error) = result {
        d.fact(Fact::Failed { priority, error });
        match error {
            Error::RateLimited { after } => {
                limited(d, env, env.now.saturating_add(after));
                let retry = match &owner {
                    Owner::Read(_) | Owner::Entry(_) => true,
                    Owner::Resource(resource) => crate::keep::active_resource(&d.keep, resource),
                    Owner::Repository(repository) => crate::keep::active_repository(&d.keep, *repository),
                };
                if retry {
                    let call = d.calls.slab.get_mut(id).expect("the refused call remains");
                    call.state = State::Queued;
                    queued(&mut d.calls, id, priority);
                    return;
                }
            }
            Error::Unavailable | Error::Timeout => {
                if wait_read(d, env, id) {
                    return;
                }
            }
            Error::Forbidden
            | Error::Missing
            | Error::MissingJob
            | Error::TooLarge
            | Error::Empty
            | Error::Full
            | Error::Exists
            | Error::NothingToMerge
            | Error::Closed
            | Error::Stale
            | Error::Conflict
            | Error::Protected
            | Error::Refused
            | Error::InvalidAnswer
            | Error::Busy => {}
        }
    }
    d.calls.slab.retire(id);
    match owner {
        Owner::Read(owner) => out.push(Request::Read { owner, result }),
        Owner::Entry(number) => crate::outbox::answered(d, env, number, result, out),
        Owner::Resource(resource) => crate::keep::answered_resource(d, env, &resource, result, out),
        Owner::Repository(repository) => crate::keep::answered_repository(d, env, repository, result, out),
    }
}
fn wait_read(d: &mut Domain, env: &Env<Limits>, id: Id<Call>) -> bool {
    let call = d.calls.slab.get_mut(id).expect("failed call remains");
    match call.owner {
        Owner::Read(_) => {}
        Owner::Entry(_) | Owner::Resource(_) | Owner::Repository(_) => return false,
    }
    match call.op {
        Op::Read(_) => {}
        Op::Write(_) => return false,
    }
    call.failures = call.failures.saturating_add(1);
    if call.failures >= env.limits.read_attempts {
        return false;
    }
    let mut delay = env.limits.backoff;
    for _ in 1..call.failures {
        delay = delay.saturating_mul(2).min(env.limits.backoff_max);
        if delay == env.limits.backoff_max {
            break;
        }
    }
    let jitter = d.rng.below(delay.as_nanos());
    let delay = delay.saturating_add(skein_lib::Duration::from_nanos(jitter)).min(env.limits.backoff_max);
    call.state = State::Waiting;
    d.alarms.arm(Alarm::ReadRetry(id), env.now.saturating_add(delay)).expect("one retry alarm per call fits");
    true
}
pub(crate) fn retry(d: &mut Domain, id: Id<Call>) {
    let call = d.calls.slab.get_mut(id).expect("retry owns a pending call");
    match call.state {
        State::Waiting => call.state = State::Queued,
        State::Queued | State::Out | State::Closed => unreachable!("retry fires only while waiting"),
    }
    let priority = call.priority;
    queued(&mut d.calls, id, priority);
}
fn limited(d: &mut Domain, env: &Env<Limits>, reset: Time) {
    d.fact(Fact::Limited { reset });
    if reset <= env.now {
        return;
    }
    let until = match d.calls.budget.reset {
        Some(at) if at >= reset => at,
        Some(_) | None => reset,
    };
    d.calls.budget.reset = Some(until);
    d.alarms.arm(Alarm::Reset, until).expect("two budget alarms fit");
}
