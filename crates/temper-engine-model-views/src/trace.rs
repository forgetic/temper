//! Traces (engine-model.md, sections 11 and 13): what runs report, kept in
//! the engine's store for a retention period, as much of each report as its
//! run's capture policy says.
//!
//! Reports are batched in memory, and a batch goes to the store once it is
//! full, or once its oldest report has waited `flush`, while fewer than
//! `appends` batches are in flight. A report that finds no room in the batch
//! while the store is behind is lost, and counted; so is a batch the store
//! fails to take. Neither is retried: traces are expendable.
//!
//! Nothing of a batch is kept once it has gone, so expiring is by time: every
//! `sweep` while records may be in the store, the views ask it to forget what
//! was reported before the retention, and stop once an expire has covered
//! every record sent. The store takes its operations in the order they are
//! asked for, so an expire covers each batch sent before it; one sent while
//! it is in flight keeps the sweep going.
//!
//! The store's transition table, as the views see it:
//!
//! ```text
//! state     event or alarm                         next      requests
//! Clean     a batch sent                           Kept      (a sweep a period on)
//! Kept      a batch sent                           Kept
//!           sweep                                  Expiring  expire what is past the retention
//! Expiring  a batch sent                           Expiring  (to sweep again)
//!           expired, of all sent, none since       Clean
//!           expired otherwise, or failed           Kept      (a sweep a period after the last)
//! ```

use alloc::boxed::Box;
use core::mem;

use temper_lib::{Env, List, Queue, Time, Token};

use crate::boundary::{Capture, Kind, Record, Request};
use crate::facts::{Fact, Kept};
use crate::limits::Limits;
use crate::model::{Model, Op};

/// The reports being batched for the store.
#[derive(Debug)]
pub(crate) struct Batch {
    pub(crate) records: List<Record>,
    /// The bytes of content the records hold.
    pub(crate) bytes: u32,
}

/// What the store may hold of what the views sent it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Store {
    /// Nothing sent is left to expire.
    Clean,
    /// Records reported up to `newest` may be kept; the next sweep falls due
    /// at `next`.
    Kept { newest: Time, next: Time },
    /// An expire of what was reported before `before`, asked at `asked`, is
    /// in flight; records up to `newest` may be kept, and `since` says
    /// whether a batch was sent after the expire, which it may not cover.
    Expiring { newest: Time, before: Time, asked: Time, since: bool },
}

/// A report of `kind` by `run`, traced as `capture` says. Says what its trace
/// kept.
pub(crate) fn record(
    model: &mut Model,
    env: &Env<Limits>,
    run: Token,
    kind: Kind,
    content: Box<[u8]>,
    capture: Capture,
    out: &mut Queue<Request>,
) -> Kept {
    let size = u32::try_from(content.len()).expect("a report within the limits, checked at the entrance");
    let (content, bytes, kept) = match capture {
        Capture::Nothing => return Kept::Nothing,
        Capture::Shape => (None, 0, Kept::Shape),
        Capture::Content => (Some(content), size, Kept::Content),
    };
    // A batch that cannot take it goes first, if the store has room.
    if !fits(&model.batch, bytes, &env.limits) {
        send(model, env, out);
    }
    if !fits(&model.batch, bytes, &env.limits) {
        return Kept::Lost;
    }
    let record = Record { run, kind, at: env.now, size, content };
    model.batch.records.push(record).expect("room in the batch, checked above");
    model.batch.bytes = model.batch.bytes.checked_add(bytes).expect("within the batch's bytes, checked above");
    kept
}

/// Whether the batch has room for one more record of `bytes` bytes.
fn fits(batch: &Batch, bytes: u32, limits: &Limits) -> bool {
    match batch.bytes.checked_add(bytes) {
        Some(total) => batch.records.room() > 0 && total <= limits.batch_bytes,
        None => false,
    }
}

/// Sends the batch to the store, if it holds anything and fewer than
/// `appends` batches are in flight.
fn send(model: &mut Model, env: &Env<Limits>, out: &mut Queue<Request>) {
    let limits = &env.limits;
    if model.appending >= limits.appends {
        return;
    }
    let Some(last) = model.batch.records.last() else {
        return;
    };
    let newest = last.at;
    // Boxed before the next batch is made: the most it holds at once is one
    // batch's list and the box it shrinks to.
    let full = mem::replace(&mut model.batch.records, List::with_capacity(0));
    let count = full.len();
    let records = full.into_boxed();
    model.batch = Batch { records: List::with_capacity(limits.records), bytes: 0 };
    let op = model.ops.insert(Op::Append).expect("room for each append in flight and the expire, twice over");
    model.appending = model.appending.checked_add(1).expect("no more appends than the limits");
    model.store = sent(model.store, newest, env);
    model.facts.push(Fact::Appending { records: count });
    out.push(Request::Append { owner: op.token(), records });
}

/// What the store may hold once a batch whose newest record was reported at
/// `newest` is sent.
fn sent(store: Store, newest: Time, env: &Env<Limits>) -> Store {
    match store {
        Store::Clean => Store::Kept { newest, next: env.now.saturating_add(env.limits.sweep) },
        Store::Kept { newest: kept, next } => Store::Kept { newest: kept.max(newest), next },
        Store::Expiring { newest: kept, before, asked, since: _ } => {
            Store::Expiring { newest: kept.max(newest), before, asked, since: true }
        }
    }
}

/// An append has ended. What it failed to keep is not sent again.
pub(crate) fn appended(model: &mut Model, done: bool) {
    model.appending = model.appending.checked_sub(1).expect("an append ends while in flight");
    model.facts.push(Fact::Appended { done });
}

/// The sweep falls due: the store is asked to forget what is past the
/// retention.
pub(crate) fn sweep(model: &mut Model, env: &Env<Limits>, out: &mut Queue<Request>) {
    model.store = match model.store {
        Store::Kept { newest, next: _ } => expire(model, env, newest, out),
        Store::Clean | Store::Expiring { .. } => {
            unreachable!("the sweep runs only while records may be kept and no expire is in flight")
        }
    };
}

/// Kept, the sweep: expire what was reported before the retention.
fn expire(model: &mut Model, env: &Env<Limits>, newest: Time, out: &mut Queue<Request>) -> Store {
    let before = Time::from_nanos(env.now.as_nanos().saturating_sub(env.limits.retention.as_nanos()));
    let op = model.ops.insert(Op::Expire).expect("room for each append in flight and the expire, twice over");
    model.facts.push(Fact::Expiring);
    out.push(Request::Expire { owner: op.token(), before });
    Store::Expiring { newest, before, asked: env.now, since: false }
}

/// The expire in flight has ended.
pub(crate) fn expired(model: &mut Model, env: &Env<Limits>, done: bool) {
    model.facts.push(Fact::Expired { done });
    model.store = match model.store {
        Store::Expiring { newest, before, asked, since } => {
            if done && !since && newest < before {
                Store::Clean
            } else {
                Store::Kept { newest, next: asked.saturating_add(env.limits.sweep) }
            }
        }
        Store::Clean | Store::Kept { .. } => unreachable!("an expire ends only while it is in flight"),
    };
}

/// What the batch and the store's state imply, after every step and alarm: a
/// batch that is due goes, if the store has room for it; the flush alarm
/// runs while the batch waits for its time, and the sweep while records may
/// be kept and no expire is in flight.
pub(crate) fn follow(model: &mut Model, env: &Env<Limits>, out: &mut Queue<Request>) {
    if let Some(first) = model.batch.records.get(0) {
        let due = first.at.saturating_add(env.limits.flush) <= env.now;
        if due || model.batch.records.room() == 0 {
            send(model, env, out);
        }
    }
    let flush = match model.batch.records.get(0) {
        Some(first) => {
            let at = first.at.saturating_add(env.limits.flush);
            if at > env.now { Some(at) } else { None }
        }
        None => None,
    };
    match flush {
        Some(at) => model.alarms.arm(Alarm::Flush, at).expect("room for each alarm"),
        None => model.alarms.cancel(Alarm::Flush),
    }
    match model.store {
        Store::Kept { newest: _, next } => model.alarms.arm(Alarm::Sweep, next).expect("room for each alarm"),
        Store::Clean | Store::Expiring { .. } => model.alarms.cancel(Alarm::Sweep),
    }
}

/// The views' own deadlines.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(crate) enum Alarm {
    /// The batch has waited long enough.
    Flush,
    /// Ask the store to forget what is past the retention.
    Sweep,
}
