//! The views child domain's state and its entry points (programming-model.md,
//! section 3).

use alloc::boxed::Box;
use core::mem;

use temper_lib::{Deadlines, Env, Id, List, Map, Queue, Slab, Time, Token};

use crate::boundary::{Event, Kind, Policy, Request, Subject};
use crate::facts::{Dropped, Fact, Facts, Loss, Lost};
use crate::limits::{self, Limits};
use crate::trace::{self, Alarm, Batch, Store};
use crate::watch::{self, Head, Watcher};

/// The views child domain's state.
#[derive(Debug)]
pub struct Domain {
    /// The runs followed, by the parent's token for each; and those turned
    /// away for want of room, with their items, as many as there is room
    /// for, so that a watch of one, and their items' watchers, are told.
    pub(crate) runs: Map<Token, Run>,
    pub(crate) unfollowed: Map<Token, Token>,
    /// The watchers, and the names of those whose watch is open: the
    /// parent's token for each.
    pub(crate) watchers: Slab<Watcher>,
    pub(crate) names: Map<Token, Id<Watcher>>,
    /// The reports being batched for the store, the appends in flight, and
    /// what the store may hold of what was sent.
    pub(crate) batch: Batch,
    pub(crate) appending: u32,
    pub(crate) store: Store,
    /// The store's operations in flight: each one's `owner` token is its
    /// handle's.
    pub(crate) ops: Slab<Op>,
    pub(crate) alarms: Deadlines<Alarm>,
    pub(crate) facts: Facts,
}

/// A run followed: the item it is for, its attempt, and its capture policy.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Run {
    pub(crate) item: Token,
    pub(crate) attempt: Token,
    pub(crate) policy: Policy,
}

/// A store operation in flight.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Op {
    /// An append of so many records.
    Append {
        records: u32,
    },
    Expire,
    /// Terminal: its terminal has come, and its slot is freed at the
    /// reclaim point.
    Ended,
}

impl Domain {
    /// A domain with room for `limits`, started at `now`. The store may hold
    /// what an earlier engine kept, so the domain starts sweeping it a period
    /// from now, and goes on until an expire has covered what was reported
    /// before it started.
    #[must_use]
    pub fn new(limits: &Limits, now: Time) -> Domain {
        let ops = limits::ops(limits).expect("worst_case accepted the limits");
        let slots = limits::slots(limits).expect("worst_case accepted the limits");
        let next = now.saturating_add(limits.sweep);
        let mut alarms = Deadlines::with_capacity(2);
        alarms.arm(Alarm::Sweep, next).expect("room for each alarm");
        Domain {
            runs: Map::with_capacity(limits.runs),
            unfollowed: Map::with_capacity(limits.runs),
            watchers: Slab::with_capacity(slots),
            names: Map::with_capacity(limits.watchers),
            batch: Batch { records: List::with_capacity(limits.records), bytes: 0 },
            appending: 0,
            store: Store::Kept { newest: now, next },
            ops: Slab::with_capacity(ops),
            alarms,
            facts: Facts::with_capacity(limits.facts),
        }
    }

    /// Runs followed.
    #[must_use]
    pub fn runs(&self) -> u32 {
        self.runs.len()
    }

    /// Watches open, those ending included until they are reclaimed.
    #[must_use]
    pub fn watchers(&self) -> u32 {
        self.watchers.len()
    }

    /// Reports batched, not yet sent to the store.
    #[must_use]
    pub fn batched(&self) -> u32 {
        self.batch.records.len()
    }

    /// Store operations in flight, ended ones included until they are
    /// reclaimed.
    #[must_use]
    pub fn ops(&self) -> u32 {
        self.ops.len()
    }

    /// Whether records may still be in the store, to expire: those sent, or
    /// those an earlier engine kept.
    #[must_use]
    pub fn is_sweeping(&self) -> bool {
        match self.store {
            Store::Clean => false,
            Store::Kept { .. } | Store::Expiring { .. } => true,
        }
    }

    /// What the views lost since the domain was made.
    #[must_use]
    pub fn lost(&self) -> Lost {
        self.facts.lost()
    }

    /// When the earliest deadline falls due.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        self.alarms.next()
    }

    /// Whether a deadline is due at `now`. While one is, the loop fires the
    /// root domain, which calls [`fire`].
    #[must_use]
    pub fn is_due(&self, now: Time) -> bool {
        match self.alarms.next() {
            Some(at) => at <= now,
            None => false,
        }
    }

    /// The oldest fact not yet drained. The parent drains them at its own
    /// pace; what does not fit meanwhile is dropped and counted.
    pub fn pop_fact(&mut self) -> Option<Fact> {
        self.facts.pop()
    }

    /// How many facts were dropped for want of room since the domain was made.
    #[must_use]
    pub fn facts_lost(&self) -> u64 {
        self.facts.lost().facts
    }

    /// The reclaim point: frees what ended in this iteration.
    pub fn reclaim(&mut self) {
        self.watchers.reclaim();
        self.ops.reclaim();
    }
}

/// The most requests one step or alarm emits under `limits`: a delivery or an
/// end to every watcher, and two batches sent, one to make room and one
/// full. The parent reserves this much room in `out`.
#[must_use]
pub const fn max_out(limits: &Limits) -> u32 {
    limits.watchers.saturating_add(2)
}

/// Handles one event, emitting at most [`max_out`] requests.
pub fn step(domain: &mut Domain, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    match event {
        Event::Started { run, attempt, item, policy } => started(domain, run, Run { item, attempt, policy }),
        Event::Reported { run, kind, content } => reported(domain, env, run, kind, content, out),
        Event::Finished { run } => finished(domain, run, out),
        Event::Phase { item, repository, phase } => changed(domain, env, item, repository, phase, out),
        Event::Watch { watcher, subject, snapshot } => watch::watch(domain, env, watcher, subject, snapshot, out),
        Event::Unwatch { watcher } => watch::unwatch(domain, watcher, out),
        Event::Delivered { watcher, done } => watch::delivered(domain, watcher, done, out),
        // A terminal that names nothing in flight is dropped.
        Event::Appended { owner, done } => match end(domain, owner) {
            Some(Op::Append { records }) => trace::appended(domain, records, done),
            Some(Op::Expire) => unreachable!("an expire ends as expired"),
            Some(Op::Ended) | None => {}
        },
        Event::Expired { owner, done } => match end(domain, owner) {
            Some(Op::Expire) => trace::expired(domain, env, done),
            Some(Op::Append { .. }) => unreachable!("an append ends as appended"),
            Some(Op::Ended) | None => {}
        },
    }
    trace::follow(domain, env, out);
}

/// Fires the earliest deadline due at `env.now`, if there is one, emitting at
/// most [`max_out`] requests. A stage fires its alarms after its input
/// events, so an append that ended in the same iteration wins over a flush
/// that fell due while the loop waited.
pub fn fire(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    let Some(alarm) = domain.alarms.expire(env.now) else {
        return;
    };
    match alarm {
        // What follows sends the batch that is due, if the store has room.
        Alarm::Flush => {}
        Alarm::Sweep => trace::sweep(domain, env, out),
    }
    trace::follow(domain, env, out);
}

/// A run's attempt was assigned: the run is followed, unless there is no
/// room for it, and then remembered as turned away, if there is room for
/// that.
fn started(domain: &mut Domain, token: Token, run: Run) {
    if domain.runs.insert(token, run).is_ok() {
        domain.unfollowed.remove(&token);
        domain.facts.push(Fact::Followed);
        return;
    }
    if domain.unfollowed.contains_key(&token) || domain.unfollowed.len() < domain.unfollowed.capacity() {
        domain.unfollowed.insert(token, run.item).expect("room for a run turned away, checked above");
    }
    domain.facts.lose(Loss::Runs, 1);
    domain.facts.push(Fact::Unfollowed);
}

/// A run reported: streamed to the watchers of the run and of its item, and
/// traced as its policy says; or, past the limits or of a run not followed,
/// dropped, and the watchers that would have had it told they missed it.
fn reported(
    domain: &mut Domain,
    env: &Env<Limits>,
    run: Token,
    kind: Kind,
    content: Box<[u8]>,
    out: &mut Queue<Request>,
) {
    let within = match u32::try_from(content.len()) {
        Ok(len) => len <= env.limits.report_bytes,
        Err(_) => false,
    };
    if let Some(&Run { item, attempt, policy }) = domain.runs.get(&run) {
        if within {
            let head = Head::Report { run, attempt, kind, at: env.now };
            let watchers = watch::offer(domain, Subject::Run(run), Subject::Item(item), head, &content, out);
            let kept = trace::record(domain, env, run, attempt, kind, content, policy.capture(kind), out);
            domain.facts.push(Fact::Reported { watchers, kept });
            return;
        }
        watch::miss(domain, Subject::Run(run), Subject::Item(item));
    } else if let Some(&item) = domain.unfollowed.get(&run) {
        watch::miss(domain, Subject::Item(item), Subject::Item(item));
    }
    let dropped = if within { Dropped::Unfollowed } else { Dropped::Oversized };
    domain.facts.lose(Loss::Reports, 1);
    domain.facts.push(Fact::Dropped { dropped });
}

/// A run ended: it is no longer followed, and its watchers end.
fn finished(domain: &mut Domain, run: Token, out: &mut Queue<Request>) {
    if domain.runs.remove(&run).is_some() {
        watch::finish(domain, run, out);
    } else {
        domain.unfollowed.remove(&run);
    }
}

/// An item's phase changed: streamed to the watchers of the item and of its
/// board.
fn changed(domain: &mut Domain, env: &Env<Limits>, item: Token, repository: u32, phase: u32, out: &mut Queue<Request>) {
    let head = Head::Phase { item, phase, at: env.now };
    let watchers = watch::offer(domain, Subject::Item(item), Subject::Board(repository), head, &[], out);
    domain.facts.push(Fact::Changed { watchers });
}

/// Ends the store operation `owner` names: what it was, or `None` if it
/// names none in flight, as a duplicate terminal does.
fn end(domain: &mut Domain, owner: Token) -> Option<Op> {
    let id = Id::from_token(owner);
    let op = domain.ops.get_mut(id)?;
    let ended = mem::replace(op, Op::Ended);
    match ended {
        Op::Append { .. } | Op::Expire => {
            domain.ops.retire(id);
            Some(ended)
        }
        Op::Ended => None,
    }
}
