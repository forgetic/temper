//! Traces (engine-domain.md, sections 11 and 13): what runs report, kept in
//! the engine's store for a retention period, as much of each report as its
//! run's capture policy says.
//!
//! Reports are batched in memory, and a batch goes to the store once it is
//! full, or once its oldest report has waited `flush`, while fewer than
//! `appends` batches are in flight. A report that finds no room in the batch
//! while the store is behind is lost, and counted; so is a batch the store
//! fails to take. Neither is retried: traces are expendable.
//!
//! The store's operations that ended in an iteration keep their slots until
//! the reclaim point. A batch or a sweep that finds none free, as when the
//! parent answers operations within the iteration, waits for the next
//! instant.
//!
//! Nothing of a batch is kept once it has gone, so expiring is by time: every
//! `sweep` while records may be in the store, the views ask it to forget what
//! was reported before the retention, and stop once an expire has covered
//! every record sent, and what was reported before the domain started, which
//! an earlier engine may have sent. The store takes its operations in the
//! order they are asked for, so an expire covers each batch sent before it;
//! one sent while it is in flight keeps the sweep going.
//!
//! The store's transition table, as the views see it:
//!
//! ```text
//! state     event or alarm                   next      requests
//! (start)                                    Kept      (a sweep a period on)
//! Clean     a batch sent                     Kept      (a sweep a period on)
//! Kept      a batch sent                     Kept
//!           sweep                            Expiring  expire what is old
//!           sweep, no operation free         Kept      (sweep next instant)
//! Expiring  a batch sent                     Expiring  (to sweep again)
//!           expired, of all sent, none since Clean
//!           expired otherwise, or failed     Kept      (a sweep a period on)
//! ```

use alloc::boxed::Box;
use core::mem;

use skein_lib::{Duration, Env, List, Queue, Time, Token};

use crate::boundary::{Capture, Kind, Record, Request};
use crate::domain::{Domain, Op};
use crate::facts::{Fact, Kept, Loss};
use crate::limits::Limits;

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

/// The next instant: when a batch or a sweep that found no operation free
/// tries again, once the reclaim point has freed those that ended.
const NEXT: Duration = Duration::from_nanos(1);

/// A report of `kind` by the attempt `attempt` of `run`, traced as `capture`
/// says. Says what its trace kept.
#[expect(clippy::too_many_arguments, reason = "a report's fields, as the step takes them")]
pub(crate) fn record(
    domain: &mut Domain,
    env: &Env<Limits>,
    run: Token,
    attempt: Token,
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
    if !fits(&domain.batch, bytes, &env.limits) {
        send(domain, env, out);
    }
    if !fits(&domain.batch, bytes, &env.limits) {
        domain.facts.lose(Loss::Records, 1);
        return Kept::Lost;
    }
    let record = Record { run, attempt, kind, at: env.now, size, content };
    domain.batch.records.push(record).expect("room in the batch, checked above");
    domain.batch.bytes = domain.batch.bytes.checked_add(bytes).expect("within the batch's bytes, checked above");
    kept
}

/// Whether the batch has room for one more record of `bytes` bytes.
fn fits(batch: &Batch, bytes: u32, limits: &Limits) -> bool {
    match batch.bytes.checked_add(bytes) {
        Some(total) => batch.records.room() > 0 && total <= limits.batch_bytes,
        None => false,
    }
}

/// Sends the batch to the store, if it holds anything, fewer than `appends`
/// batches are in flight, and an operation is free.
fn send(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    let limits = &env.limits;
    if domain.appending >= limits.appends || domain.ops.is_full() {
        return;
    }
    let Some(last) = domain.batch.records.last() else {
        return;
    };
    let newest = last.at;
    // Boxed before the next batch is made: the most it holds at once is one
    // batch's list and the box it shrinks to.
    let full = mem::replace(&mut domain.batch.records, List::with_capacity(0));
    let count = full.len();
    let records = full.into_boxed();
    domain.batch = Batch { records: List::with_capacity(limits.records), bytes: 0 };
    let op = domain.ops.insert(Op::Append { records: count }).expect("an operation free, checked above");
    domain.appending = domain.appending.checked_add(1).expect("no more appends than the limits");
    domain.store = sent(domain.store, newest, env);
    domain.facts.push(Fact::Appending { records: count });
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

/// An append of `records` records has ended. What it failed to keep is not
/// sent again, and is counted lost.
pub(crate) fn appended(domain: &mut Domain, records: u32, done: bool) {
    domain.appending = domain.appending.checked_sub(1).expect("an append ends while in flight");
    if !done {
        domain.facts.lose(Loss::Records, u64::from(records));
    }
    domain.facts.push(Fact::Appended { done });
}

/// The sweep falls due: the store is asked to forget what is past the
/// retention, or, with no operation free, the sweep waits for the next
/// instant.
pub(crate) fn sweep(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    domain.store = match domain.store {
        Store::Kept { newest, next: _ } if domain.ops.is_full() => {
            Store::Kept { newest, next: env.now.saturating_add(NEXT) }
        }
        Store::Kept { newest, next: _ } => expire(domain, env, newest, out),
        Store::Clean | Store::Expiring { .. } => {
            unreachable!("the sweep runs only while records may be kept and no expire is in flight")
        }
    };
}

/// Kept, the sweep: expire what was reported before the retention.
fn expire(domain: &mut Domain, env: &Env<Limits>, newest: Time, out: &mut Queue<Request>) -> Store {
    let before = Time::from_nanos(env.now.as_nanos().saturating_sub(env.limits.retention.as_nanos()));
    let op = domain.ops.insert(Op::Expire).expect("an operation free, checked by the sweep");
    domain.facts.push(Fact::Expiring);
    out.push(Request::Expire { owner: op.token(), before });
    Store::Expiring { newest, before, asked: env.now, since: false }
}

/// The expire in flight has ended.
pub(crate) fn expired(domain: &mut Domain, env: &Env<Limits>, done: bool) {
    domain.facts.push(Fact::Expired { done });
    domain.store = match domain.store {
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
/// runs while the batch waits for its time, or, due, for an operation to be
/// free (an append's end sends one that waits for the store); and the sweep
/// while records may be kept and no expire is in flight.
pub(crate) fn follow(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    if let Some(first) = domain.batch.records.get(0) {
        let due = first.at.saturating_add(env.limits.flush) <= env.now;
        if due || domain.batch.records.room() == 0 {
            send(domain, env, out);
        }
    }
    let flush = match domain.batch.records.get(0) {
        Some(first) => {
            let at = first.at.saturating_add(env.limits.flush);
            let ready = at <= env.now || domain.batch.records.room() == 0;
            if !ready {
                Some(at)
            } else if domain.appending < env.limits.appends {
                // Ready, and not sent: no operation was free.
                Some(env.now.saturating_add(NEXT))
            } else {
                None
            }
        }
        None => None,
    };
    match flush {
        Some(at) => domain.alarms.arm(Alarm::Flush, at).expect("room for each alarm"),
        None => domain.alarms.cancel(Alarm::Flush),
    }
    match domain.store {
        Store::Kept { newest: _, next } => domain.alarms.arm(Alarm::Sweep, next).expect("room for each alarm"),
        Store::Clean | Store::Expiring { .. } => domain.alarms.cancel(Alarm::Sweep),
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
