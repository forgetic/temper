//! Segment coverage and componentwise authority order (domain/authority.md, 4–5).

use alloc::boxed::Box;

use crate::{Authority, Grant, Last, Name, Pattern};

/// On one connector, granting `kind` also grants `implies`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Implication {
    pub connector: u16,
    pub kind: u16,
    pub implies: u16,
}

/// A connector-scoped preorder. Equality is implicit; all other transitive
/// consequences must be present. Cycles and duplicate entries are permitted.
/// The private table can only be made by validating bounded configuration.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Implies {
    pairs: Box<[Implication]>,
}

impl Implies {
    /// Refuses past the configured pair limit or if the table is not
    /// transitively closed. Validation scans at most `max_pairs` cubed;
    /// queries scan at most `max_pairs`, and neither allocates.
    #[must_use]
    pub fn new(pairs: Box<[Implication]>, max_pairs: u32) -> Option<Implies> {
        if pairs.len() > usize::try_from(max_pairs).ok()? {
            return None;
        }
        let table = Implies { pairs };
        for first in &table.pairs {
            for second in &table.pairs {
                if first.connector == second.connector
                    && first.implies == second.kind
                    && !table.allows(first.connector, second.implies, first.kind)
                {
                    return None;
                }
            }
        }
        Some(table)
    }

    /// Whether a grant of `given` includes `needed` on this connector.
    #[must_use]
    pub fn allows(&self, connector: u16, needed: u16, given: u16) -> bool {
        if needed == given {
            return true;
        }
        for pair in &self.pairs {
            if pair.connector == connector && pair.kind == given && pair.implies == needed {
                return true;
            }
        }
        false
    }
}

/// Whether the pattern includes this name, compared segment by segment.
#[must_use]
pub fn pattern_covers(pattern: &Pattern, name: &Name) -> bool {
    if name.segments.get(..pattern.segments.len()) != Some(pattern.segments.as_ref()) {
        return false;
    }
    match &pattern.last {
        Last::None => name.segments.len() == pattern.segments.len(),
        Last::Exact(last) => match name.segments.get(pattern.segments.len()) {
            Some(segment) => segment == last,
            None => false,
        },
        Last::Open(last) => match name.segments.get(pattern.segments.len()) {
            Some(segment) => segment.starts_with(last),
            None => false,
        },
    }
}

/// Whether every name covered by `a` is covered by `b`, without enumerating names.
#[must_use]
pub fn pattern_at_most(a: &Pattern, b: &Pattern) -> bool {
    match &b.last {
        Last::None => match &a.last {
            Last::None => a.segments == b.segments,
            Last::Exact(_) | Last::Open(_) => false,
        },
        Last::Exact(last) => terminal_at_most(a, b, last, false),
        Last::Open(last) => terminal_at_most(a, b, last, true),
    }
}

fn terminal_at_most(a: &Pattern, b: &Pattern, last: &[u8], open: bool) -> bool {
    if a.segments.get(..b.segments.len()) != Some(b.segments.as_ref()) {
        return false;
    }
    if let Some(segment) = a.segments.get(b.segments.len()) {
        return if open { segment.starts_with(last) } else { segment.as_ref() == last };
    }
    match &a.last {
        Last::None => false,
        Last::Exact(segment) => {
            if open {
                segment.starts_with(last)
            } else {
                segment.as_ref() == last
            }
        }
        Last::Open(segment) => open && segment.starts_with(last),
    }
}

/// Every resource and kind of `a` must be included by this one grant `b`.
#[must_use]
pub fn grant_at_most(a: &Grant, b: &Grant, implies: &Implies) -> bool {
    a.connector == b.connector && implies.allows(a.connector, a.kind, b.kind) && pattern_at_most(&a.pattern, &b.pattern)
}

/// Whether a grant includes one named resource of the requested connector and kind.
#[must_use]
pub fn grant_covers(grant: &Grant, connector: u16, kind: u16, name: &Name, implies: &Implies) -> bool {
    grant.connector == connector && implies.allows(connector, kind, grant.kind) && pattern_covers(&grant.pattern, name)
}

/// Whether every part of `a` is at most `b`. Duplicate or reordered grants
/// and executors need not make equal values to make equivalent authority.
#[must_use]
pub fn at_most(a: &Authority, b: &Authority, implies: &Implies) -> bool {
    if a.tools.0 & !b.tools.0 != 0
        || a.notes.0 & !b.notes.0 != 0
        || a.delegation.tasks > b.delegation.tasks
        || a.delegation.depth > b.delegation.depth
        || a.budget.spend > b.budget.spend
    {
        return false;
    }
    let deadline_fits = match a.budget.deadline {
        Some(deadline) => match b.budget.deadline {
            Some(ceiling) => deadline <= ceiling,
            None => true,
        },
        None => b.budget.deadline.is_none(),
    };
    if !deadline_fits {
        return false;
    }
    for kind in &a.delegation.kinds {
        if !b.delegation.kinds.contains(kind) {
            return false;
        }
    }
    for grant in &a.grants {
        let mut covered = false;
        for ceiling in &b.grants {
            if grant_at_most(grant, ceiling, implies) {
                covered = true;
                break;
            }
        }
        if !covered {
            return false;
        }
    }
    true
}
