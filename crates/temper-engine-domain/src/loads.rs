//! Paged root loads, one terminal per issued token (domain/engine.md, 5.3).
//! The caller dispatches a load only after the commits it reads are answered.
use crate::{Key, Range, Record};
use alloc::boxed::Box;
use core::mem::size_of;
use skein_lib::{Id, List, Queue, Slab, Token};

/// Startup bounds for paged store reads (domain/engine.md, section 5.3).
/// The root supplies them; the protocol enforces the same decoded-page bound.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    /// Maximum issued loads, including abandoned IO awaiting its terminal.
    pub loads: u32,
    /// Maximum rows requested and accepted in one incoming page.
    pub rows: u32,
    /// Soft kept-prefix bound, counting record slots and their owned bytes.
    pub bytes: u32,
    /// Hard decoded store-answer bound, enforced by the store protocol too.
    /// The kept prefix can have a smaller budget than its incoming page.
    pub reply_bytes: u32,
    /// Soft per-turn transcript bound; the first oversized row cuts the prefix.
    pub transcript_bytes: u32,
}

/// Explicit omission from a validated page (domain/engine.md, section 5.3).
/// A load waiter receives this with the kept prefix; it must handle zero progress.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Cut {
    /// Number of omitted rows, bounded by the issued page's row limit.
    pub rows: u32,
    /// Omitted record slots and owned bytes, bounded by the decoded-page limit.
    pub bytes: u64,
}

/// Terminal load failure delivered to its root waiter (domain/engine.md, 5.3).
/// A malformed page is rejected whole before allocating any kept prefix.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Failure {
    /// The store ended its issued load without rows.
    Store,
    /// The page exceeded the issued row count or its count was unrepresentable.
    Rows,
    /// A row belongs outside the issued key range.
    Range,
    /// Keys are repeated, unordered or at/before the exclusive starting key.
    Order,
    /// Continuation does not identify the last returned row in its range.
    Cursor,
    /// Decoded slots and owned bytes overflow or exceed the hard page bound.
    Bytes,
}

/// Store IO and root-waiter handoffs (domain/engine.md, section 5.3).
/// Every issued load ends once; abandonment retains IO ownership until then.
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    /// Root to store; ends through `loaded` or `unloaded` with this owner.
    /// The caller waits for prerequisite commits before dispatching it.
    Load {
        /// Generational IO identity, fenced after its terminal and reclaim.
        owner: Token,
        /// Closed key range that every returned row must belong to.
        range: Range,
        /// Exclusive starting key in the range, or its beginning.
        after: Option<Key>,
        /// Positive row demand, no larger than `Limits::rows`.
        most: u32,
        /// Hard decoded-page bytes, equal to `Limits::reply_bytes`.
        bytes: u32,
    },
    /// One successful terminal to the root waiter, unless abandoned.
    Loaded {
        /// Root-owned identity passed unchanged from the admitted request.
        waiter: Token,
        /// Owned, validated whole-row prefix within the soft byte limits.
        rows: Box<[Record]>,
        /// Exclusive continuation; a cut may leave it at the original key.
        next: Option<Key>,
        /// Explicit omissions, including a zero-row kept prefix if necessary.
        cut: Option<Cut>,
    },
    /// One failed terminal to the root waiter, unless abandoned.
    Unloaded {
        /// Root-owned identity passed unchanged from the admitted request.
        waiter: Token,
        /// Store failure or whole-page validation refusal.
        failure: Failure,
    },
}

#[derive(Debug)]
struct Entry {
    waiter: Token,
    range: Range,
    after: Option<Key>,
    most: u32,
    phase: Phase,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    Waiting,
    Abandoned,
    Closed,
}

/// Bounded in-flight page ownership (domain/engine.md, section 5.3).
/// It keeps waiter/range fences, never store rows or journal durability state.
#[derive(Debug)]
pub struct Loads {
    limits: Limits,
    entries: Slab<Entry>,
}

impl Loads {
    /// Allocate fixed load slots from validated startup limits (domain/engine.md, 5.3).
    /// Invalid limits fail at startup; per-request saturation is a refusal.
    #[must_use]
    pub fn new(limits: &Limits) -> Loads {
        assert!(worst_case(limits).is_some(), "valid root load limits");
        Loads { limits: *limits, entries: Slab::with_capacity(limits.loads) }
    }

    /// Shell completion fence: no issued page awaits its terminal or reclaim.
    /// Abandoned IO remains counted until its actual answer (domain/engine.md, 5.3).
    #[must_use]
    pub fn quiescent(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Admit one root request or refuse before issuing IO (domain/engine.md, 5.3).
/// Reserves one output; the issued owner ends through `loaded` or `unloaded`.
/// Journal pressure is independent: store terminals are always accepted.
pub fn begin(
    domain: &mut Loads,
    waiter: Token,
    range: Range,
    after: Option<Key>,
    most: u32,
    out: &mut Queue<Request>,
) -> Option<Token> {
    assert!(out.room() >= 1, "one root load output reserved");
    if most == 0 || most > domain.limits.rows || !valid_range(range) {
        return None;
    }
    if let Some(key) = after
        && !range.contains(key)
    {
        return None;
    }
    let entry = Entry { waiter, range, after, most, phase: Phase::Waiting };
    let id = domain.entries.insert(entry).ok()?;
    let owner = id.token();
    out.push(Request::Load { owner, range, after, most, bytes: domain.limits.reply_bytes });
    Some(owner)
}

/// Abandon delivery to a waiter (domain/engine.md, section 5.3).
/// Its issued IO still occupies a slot until the store's actual terminal;
/// this call neither emits a terminal nor releases that ownership.
pub fn abandon(domain: &mut Loads, owner: Token) {
    if let Some(entry) = domain.entries.get_mut(Id::from_token(owner)) {
        match entry.phase {
            Phase::Waiting => entry.phase = Phase::Abandoned,
            Phase::Abandoned | Phase::Closed => {}
        }
    }
}

/// Accept the store's failed terminal (domain/engine.md, section 5.3).
/// Reserve one output. Notify a live waiter once; discard abandoned/late answers.
pub fn unloaded(domain: &mut Loads, owner: Token, out: &mut Queue<Request>) {
    assert!(out.room() >= 1, "one root load terminal reserved");
    let id = Id::from_token(owner);
    if let Some(entry) = domain.entries.get_mut(id) {
        match entry.phase {
            Phase::Waiting => out.push(Request::Unloaded { waiter: entry.waiter, failure: Failure::Store }),
            Phase::Abandoned => {}
            Phase::Closed => return,
        }
        entry.phase = Phase::Closed;
        domain.entries.retire(id);
    }
}

/// Accept the store's owned page terminal (domain/engine.md, section 5.3).
/// Reserve one output. Validate the whole page before moving its bounded prefix;
/// malformed input ends as failure and duplicate or abandoned input is dropped.
pub fn loaded(domain: &mut Loads, owner: Token, rows: Box<[Record]>, next: Option<Key>, out: &mut Queue<Request>) {
    assert!(out.room() >= 1, "one root load terminal reserved");
    let id = Id::from_token(owner);
    let Some(entry) = domain.entries.get_mut(id) else {
        return;
    };
    match entry.phase {
        Phase::Abandoned => {
            entry.phase = Phase::Closed;
            domain.entries.retire(id);
            return;
        }
        Phase::Closed => return,
        Phase::Waiting => {}
    }
    let checked = check(entry, &domain.limits, &rows, next);
    let waiter = entry.waiter;
    entry.phase = Phase::Closed;
    domain.entries.retire(id);
    match checked {
        Err(failure) => out.push(Request::Unloaded { waiter, failure }),
        Ok(page) => {
            if page.cut.is_none() {
                out.push(Request::Loaded { waiter, rows, next, cut: None });
                return;
            }
            // Capacity equals the kept prefix. No partial list is shrunk,
            // and owned rows are moved without cloning transcript bytes.
            let mut kept = List::with_capacity(page.keep);
            for row in rows {
                if kept.len() < page.keep {
                    kept.push(row).expect("checked prefix capacity");
                }
            }
            out.push(Request::Loaded { waiter, rows: kept.into_boxed(), next: page.next, cut: page.cut });
        }
    }
}

/// Reclaim retired terminal slots at the iteration boundary (domain/engine.md, 5.3).
/// Abandoned loads still awaiting their store terminal cannot be reclaimed.
pub fn reclaim(domain: &mut Loads) {
    domain.entries.reclaim();
}

#[derive(Debug)]
struct Page {
    keep: u32,
    next: Option<Key>,
    cut: Option<Cut>,
}

fn check(entry: &Entry, limits: &Limits, rows: &[Record], next: Option<Key>) -> Result<Page, Failure> {
    let Ok(count) = u32::try_from(rows.len()) else {
        return Err(Failure::Rows);
    };
    if count > entry.most {
        return Err(Failure::Rows);
    }
    let mut previous = entry.after;
    let mut last_kept = entry.after;
    let mut bytes = 0_u64;
    let mut removed_bytes = 0_u64;
    let mut keep = 0_u32;
    let mut cutting = false;
    for row in rows {
        let key = row.key();
        if !entry.range.contains(key) {
            return Err(Failure::Range);
        }
        if let Some(old) = previous
            && key <= old
        {
            return Err(Failure::Order);
        }
        previous = Some(key);
        let owned = crate::store::record_bytes(row).ok_or(Failure::Bytes)?;
        let size = u64::try_from(size_of::<Record>())
            .expect("record slot fits u64")
            .checked_add(owned)
            .ok_or(Failure::Bytes)?;
        let total = bytes.checked_add(size).ok_or(Failure::Bytes)?;
        if cutting || owned > u64::from(limits.transcript_bytes) || total > u64::from(limits.bytes) {
            cutting = true;
            removed_bytes = removed_bytes.checked_add(size).ok_or(Failure::Bytes)?;
        } else {
            bytes = total;
            keep = keep.checked_add(1).expect("bounded page count");
            last_kept = Some(key);
        }
    }
    if let Some(cursor) = next {
        if rows.is_empty() || Some(cursor) != previous || !entry.range.contains(cursor) {
            return Err(Failure::Cursor);
        }
        match entry.range {
            Range::Deployment | Range::TaskResult { .. } => return Err(Failure::Cursor),
            Range::Turns { .. } | Range::Tasks | Range::People | Range::RunProofs => {}
        }
    }
    if bytes.checked_add(removed_bytes).ok_or(Failure::Bytes)? > u64::from(limits.reply_bytes) {
        return Err(Failure::Bytes);
    }
    let cut = if cutting {
        Some(Cut { rows: count.checked_sub(keep).expect("prefix is within page"), bytes: removed_bytes })
    } else {
        None
    };
    Ok(Page { keep, next: if cutting { last_kept } else { next }, cut })
}

fn valid_range(range: Range) -> bool {
    match range {
        Range::Deployment | Range::Tasks | Range::People | Range::RunProofs => true,
        Range::TaskResult { task } => task != 0,
        Range::Turns { task, attempt } => task != 0 && attempt != 0,
    }
}

/// Startup heap bound, including decoded IO and temporary prefix slots
/// (domain/engine.md, section 5.3). Invalid limits or arithmetic return `None`.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    if limits.loads == 0
        || limits.rows == 0
        || limits.reply_bytes < limits.bytes
        || u64::from(limits.bytes) < u64::try_from(size_of::<Record>()).ok()?
    {
        return None;
    }
    Slab::<Entry>::worst_case(limits.loads)?
        .checked_add(List::<Record>::worst_case(limits.rows)?)?
        .checked_add(u64::from(limits.reply_bytes))
}
