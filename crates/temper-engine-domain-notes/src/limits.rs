//! Bounds for notes (jig's domain/engine.md, sections 10 and 13).

use core::mem::size_of;

use skein_lib::List;

use crate::boundary::{Entry, Line, Record};
use crate::domain::Pending;

/// The parent's fixed bounds for notes and store pages.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Limits {
    /// Entries admitted in one scope.
    pub entries_per_scope: u32,
    /// Largest connector pattern, in bytes.
    pub pattern_bytes: u32,
    /// Largest one-line description, in bytes.
    pub description_bytes: u32,
    /// Largest body, in bytes.
    pub body_bytes: u32,
    /// Task references in one entry.
    pub references: u32,
    /// Records in one store load page.
    pub load_rows: u32,
}

/// The maximum heap the pending call and incoming page can hold, or `None`
/// when a bound cannot be represented. Outputs move to the parent.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    if limits.entries_per_scope == 0 || limits.load_rows == 0 {
        return None;
    }
    let pending = u64::try_from(size_of::<Pending>())
        .ok()?
        .checked_add(u64::from(limits.pattern_bytes))?
        .checked_add(u64::from(limits.description_bytes))?
        .checked_add(u64::from(limits.body_bytes))?
        .checked_add(List::<u64>::worst_case(limits.references)?)?;
    let entry = u64::try_from(size_of::<Entry>())
        .ok()?
        .checked_add(u64::from(limits.pattern_bytes))?
        .checked_add(u64::from(limits.description_bytes))?
        .checked_add(u64::from(limits.body_bytes))?
        .checked_add(List::<u64>::worst_case(limits.references)?)?;
    let line = u64::try_from(size_of::<Line>())
        .ok()?
        .checked_add(u64::from(limits.pattern_bytes))?
        .checked_add(u64::from(limits.description_bytes))?;
    let page = List::<Record>::worst_case(limits.load_rows)?
        .checked_add(u64::from(limits.load_rows).checked_mul(entry.max(line))?)?;
    // An entry lookup clones its single row while deciding the write.
    pending.checked_add(page)?.checked_add(entry)
}
