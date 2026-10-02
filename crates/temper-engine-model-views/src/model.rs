//! The views sub-model's state and its entry points (programming-style.md,
//! section 3).

use alloc::boxed::Box;

use temper_lib::{Deadlines, Env, Id, List, Map, Queue, Slab, Time, Token};

use crate::boundary::{Event, Kind, Phase, Policy, Request, Subject};
use crate::facts::{Dropped, Fact, Facts};
use crate::limits::{self, Limits};
use crate::trace::{self, Alarm, Batch, Store};
use crate::watch::{self, Head, Watcher};

/// The views sub-model's state.
#[derive(Debug)]
pub struct Model {
    /// The runs followed, by the parent's token for each.
    pub(crate) runs: Map<Token, Run>,
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

/// A run followed: the item it is for, and its capture policy.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Run {
    pub(crate) item: Token,
    pub(crate) policy: Policy,
}

/// A store operation in flight.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Op {
    Append,
    Expire,
}

impl Model {
    /// A model with room for `limits`.
    #[must_use]
    pub fn new(limits: &Limits) -> Model {
        let ops = limits::ops(limits).expect("worst_case accepted the limits");
        Model {
            runs: Map::with_capacity(limits.runs),
            watchers: Slab::with_capacity(limits::slots(limits).expect("worst_case accepted the limits")),
            names: Map::with_capacity(limits.watchers),
            batch: Batch { records: List::with_capacity(limits.records), bytes: 0 },
            appending: 0,
            store: Store::Clean,
            ops: Slab::with_capacity(ops),
            alarms: Deadlines::with_capacity(2),
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

    /// Whether records the views sent may still be in the store, to expire.
    #[must_use]
    pub fn is_sweeping(&self) -> bool {
        match self.store {
            Store::Clean => false,
            Store::Kept { .. } | Store::Expiring { .. } => true,
        }
    }

    /// When the earliest deadline falls due.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        self.alarms.next()
    }

    /// Whether a deadline is due at `now`. While one is, the loop fires the
    /// top-level model, which calls [`fire`].
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

    /// How many facts were dropped for want of room since the model was made.
    #[must_use]
    pub fn facts_lost(&self) -> u64 {
        self.facts.lost()
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
pub fn step(model: &mut Model, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    match event {
        Event::Started { run, item, policy } => started(model, run, item, policy),
        Event::Reported { run, kind, content } => reported(model, env, run, kind, content, out),
        Event::Finished { run } => finished(model, run, out),
        Event::Phase { item, repository, phase } => changed(model, env, item, repository, phase, out),
        Event::Watch { watcher, subject } => watch::watch(model, env, watcher, subject, out),
        Event::Unwatch { watcher } => watch::unwatch(model, watcher, out),
        Event::Delivered { watcher } => watch::delivered(model, watcher, out),
        // A terminal that names no operation in flight is dropped.
        Event::Appended { owner, done } => match end(model, owner) {
            Some(Op::Append) => trace::appended(model, done),
            Some(Op::Expire) => unreachable!("an expire ends as expired"),
            None => {}
        },
        Event::Expired { owner, done } => match end(model, owner) {
            Some(Op::Expire) => trace::expired(model, env, done),
            Some(Op::Append) => unreachable!("an append ends as appended"),
            None => {}
        },
    }
    trace::follow(model, env, out);
}

/// Fires the earliest deadline due at `env.now`, if there is one, emitting at
/// most [`max_out`] requests. A stage fires its alarms after its input
/// events, so an append that ended in the same iteration wins over a flush
/// that fell due while the loop waited.
pub fn fire(model: &mut Model, env: &Env<Limits>, out: &mut Queue<Request>) {
    let Some(alarm) = model.alarms.expire(env.now) else {
        return;
    };
    match alarm {
        // What follows sends the batch that is due, if the store has room.
        Alarm::Flush => {}
        Alarm::Sweep => trace::sweep(model, env, out),
    }
    trace::follow(model, env, out);
}

/// A run was assigned: it is followed, unless there is no room for it.
fn started(model: &mut Model, run: Token, item: Token, policy: Policy) {
    let fact = match model.runs.insert(run, Run { item, policy }) {
        Ok(_) => Fact::Followed,
        Err(_) => Fact::Unfollowed,
    };
    model.facts.push(fact);
}

/// A run reported: streamed to the watchers of the run and of its item, and
/// traced as its policy says.
fn reported(
    model: &mut Model,
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
    if !within {
        model.facts.push(Fact::Dropped { dropped: Dropped::Oversized });
        return;
    }
    let Some(&Run { item, policy }) = model.runs.get(&run) else {
        model.facts.push(Fact::Dropped { dropped: Dropped::Unfollowed });
        return;
    };
    let head = Head::Report { run, kind, at: env.now };
    let watchers = watch::offer(model, Subject::Run(run), Subject::Item(item), head, &content, out);
    let kept = trace::record(model, env, run, kind, content, policy.capture(kind), out);
    model.facts.push(Fact::Reported { watchers, kept });
}

/// A run ended: it is no longer followed, and its watchers end.
fn finished(model: &mut Model, run: Token, out: &mut Queue<Request>) {
    if model.runs.remove(&run).is_some() {
        watch::finish(model, run, out);
    }
}

/// An item's phase changed: streamed to the watchers of the item and of its
/// board.
fn changed(model: &mut Model, env: &Env<Limits>, item: Token, repository: u32, phase: Phase, out: &mut Queue<Request>) {
    let head = Head::Phase { item, phase, at: env.now };
    let watchers = watch::offer(model, Subject::Item(item), Subject::Board(repository), head, &[], out);
    model.facts.push(Fact::Changed { watchers });
}

/// Ends the store operation `owner` names: what it was, or `None` if it
/// names none in flight.
fn end(model: &mut Model, owner: Token) -> Option<Op> {
    let id = Id::from_token(owner);
    let op = *model.ops.get(id)?;
    model.ops.retire(id);
    Some(op)
}
