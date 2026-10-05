//! Paged root loads, one terminal per issued token (domain/engine.md, 5.3).
//! The caller dispatches a load only after the commits it reads are answered.
use crate::{Key, Range, Record};
use alloc::boxed::Box;
use core::mem::size_of;
use skein_lib::{Id, List, Queue, Slab, Token};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    pub loads: u32,
    pub rows: u32,
    /// Fixed record slots and their owned transcript bytes, together.
    pub bytes: u32,
    /// Hard decoded store-answer bound, enforced by the store protocol too.
    /// The kept prefix can have a smaller budget than its incoming page.
    pub reply_bytes: u32,
    pub transcript_bytes: u32,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Cut {
    pub rows: u32,
    pub bytes: u64,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Failure {
    Store,
    Rows,
    Range,
    Order,
    Cursor,
    Bytes,
}
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    Load { owner: Token, range: Range, after: Option<Key>, most: u32, bytes: u32 },
    Loaded { waiter: Token, rows: Box<[Record]>, next: Option<Key>, cut: Option<Cut> },
    Unloaded { waiter: Token, failure: Failure },
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
#[derive(Debug)]
pub struct Loads {
    limits: Limits,
    entries: Slab<Entry>,
}
impl Loads {
    #[must_use]
    pub fn new(l: &Limits) -> Loads {
        assert!(worst_case(l).is_some(), "valid root load limits");
        Loads { limits: *l, entries: Slab::with_capacity(l.loads) }
    }
}
/// Refuses before issuing anything. Admission is independent of journal
/// pressure: store terminals must remain accepted while commits are full.
pub fn begin(
    d: &mut Loads,
    waiter: Token,
    range: Range,
    after: Option<Key>,
    most: u32,
    out: &mut Queue<Request>,
) -> Option<Token> {
    assert!(out.room() >= 1, "one root load output reserved");
    if most == 0 || most > d.limits.rows || !valid_range(range) {
        return None;
    }
    if let Some(key) = after
        && !range.contains(key)
    {
        return None;
    }
    let entry = Entry { waiter, range, after, most, phase: Phase::Waiting };
    let id = d.entries.insert(entry).ok()?;
    let owner = id.token();
    out.push(Request::Load { owner, range, after, most, bytes: d.limits.reply_bytes });
    Some(owner)
}
/// An abandoned issued IO still occupies its slot until its actual terminal.
/// It cannot be reclaimed as if the store had already answered.
pub fn abandon(d: &mut Loads, owner: Token) {
    if let Some(entry) = d.entries.get_mut(Id::from_token(owner)) {
        match entry.phase {
            Phase::Waiting => entry.phase = Phase::Abandoned,
            Phase::Abandoned | Phase::Closed => {}
        }
    }
}
pub fn unloaded(d: &mut Loads, owner: Token, out: &mut Queue<Request>) {
    assert!(out.room() >= 1, "one root load terminal reserved");
    let id = Id::from_token(owner);
    if let Some(entry) = d.entries.get_mut(id) {
        match entry.phase {
            Phase::Waiting => out.push(Request::Unloaded { waiter: entry.waiter, failure: Failure::Store }),
            Phase::Abandoned => {}
            Phase::Closed => return,
        }
        entry.phase = Phase::Closed;
        d.entries.retire(id);
    }
}
pub fn loaded(d: &mut Loads, owner: Token, rows: Box<[Record]>, next: Option<Key>, out: &mut Queue<Request>) {
    assert!(out.room() >= 1, "one root load terminal reserved");
    let id = Id::from_token(owner);
    let Some(entry) = d.entries.get_mut(id) else {
        return;
    };
    match entry.phase {
        Phase::Abandoned => {
            entry.phase = Phase::Closed;
            d.entries.retire(id);
            return;
        }
        Phase::Closed => return,
        Phase::Waiting => {}
    }
    let checked = check(entry, &d.limits, &rows, next);
    let waiter = entry.waiter;
    entry.phase = Phase::Closed;
    d.entries.retire(id);
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
pub fn reclaim(d: &mut Loads) {
    d.entries.reclaim();
}
#[derive(Debug)]
struct Page {
    keep: u32,
    next: Option<Key>,
    cut: Option<Cut>,
}
fn check(entry: &Entry, l: &Limits, rows: &[Record], next: Option<Key>) -> Result<Page, Failure> {
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
        let owned = match row {
            Record::Deployment(_) => 0,
            Record::Turn(turn) => match u64::try_from(turn.transcript.len()) {
                Ok(bytes) => bytes,
                Err(_) => return Err(Failure::Bytes),
            },
        };
        let size = u64::try_from(size_of::<Record>())
            .expect("record slot fits u64")
            .checked_add(owned)
            .ok_or(Failure::Bytes)?;
        let total = bytes.checked_add(size).ok_or(Failure::Bytes)?;
        if cutting || owned > u64::from(l.transcript_bytes) || total > u64::from(l.bytes) {
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
            Range::Deployment => return Err(Failure::Cursor),
            Range::Turns { .. } => {}
        }
    }
    if bytes.checked_add(removed_bytes).ok_or(Failure::Bytes)? > u64::from(l.reply_bytes) {
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
        Range::Deployment => true,
        Range::Turns { task, attempt } => task != 0 && attempt != 0,
    }
}
#[must_use]
pub fn worst_case(l: &Limits) -> Option<u64> {
    if l.loads == 0
        || l.rows == 0
        || l.reply_bytes < l.bytes
        || u64::from(l.bytes) < u64::try_from(size_of::<Record>()).ok()?
    {
        return None;
    }
    Slab::<Entry>::worst_case(l.loads)?
        .checked_add(List::<Record>::worst_case(l.rows)?)?
        .checked_add(u64::from(l.reply_bytes))
}
