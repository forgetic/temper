//! Segment coverage and componentwise authority order (domain/authority.md, 4–5).

use alloc::boxed::Box;

use skein_lib::Queue;

use crate::{Authority, Grant, Last, Name, Numbers, Pattern, left};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Lack {
    Tools,
    Grants,
    Executors,
    Tasks,
    Depth,
    Spend,
    Deadline,
    Notes,
}

/// Every independent component that failed; a check can name all of them
/// in one bounded finding without losing the stricter answer.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[expect(clippy::struct_excessive_bools, reason = "these are independent component deficits, not lifecycle state")]
pub struct Lacks {
    pub tools: bool,
    pub grants: bool,
    pub executors: bool,
    pub tasks: bool,
    pub depth: bool,
    pub spend: bool,
    pub deadline: bool,
    pub notes: bool,
}

impl Lacks {
    #[must_use]
    pub const fn is_empty(self) -> bool {
        !(self.tools
            || self.grants
            || self.executors
            || self.tasks
            || self.depth
            || self.spend
            || self.deadline
            || self.notes)
    }
}

pub const FITS_MAX_OUT: u32 = 8;

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

    #[must_use]
    pub fn len(&self) -> u32 {
        u32::try_from(self.pairs.len()).expect("constructor bounds pairs by a u32")
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
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
    differences(a, b, b.delegation.tasks, b.delegation.depth, b.budget.spend, implies).is_empty()
}

/// Whether the child's authority fits, with spend and lifetime task
/// capacity taken from separate current inputs (domain/authority.md, 5).
/// The caller reserves `FITS_MAX_OUT` queue slots. A batch additionally
/// consumes one task slot for each immediate child and sums reservations.
#[must_use]
pub fn fits(
    child: &Authority,
    creator: &Authority,
    numbers: &Numbers,
    tasks_left: u32,
    implies: &Implies,
    lacks: &mut Queue<Lack>,
) -> bool {
    assert!(lacks.room() >= FITS_MAX_OUT, "caller reserves every fitting finding");
    let parts = fit_lacks(child, creator, numbers, tasks_left, implies);
    if parts.tools {
        lacks.push(Lack::Tools);
    }
    if parts.grants {
        lacks.push(Lack::Grants);
    }
    if parts.executors {
        lacks.push(Lack::Executors);
    }
    if parts.tasks {
        lacks.push(Lack::Tasks);
    }
    if parts.depth {
        lacks.push(Lack::Depth);
    }
    if parts.spend {
        lacks.push(Lack::Spend);
    }
    if parts.deadline {
        lacks.push(Lack::Deadline);
    }
    if parts.notes {
        lacks.push(Lack::Notes);
    }
    parts.is_empty()
}

pub(crate) fn fit_lacks(
    child: &Authority,
    creator: &Authority,
    numbers: &Numbers,
    tasks_left: u32,
    implies: &Implies,
) -> Lacks {
    let mut parts = differences(
        child,
        creator,
        tasks_left.min(creator.delegation.tasks),
        creator.delegation.depth.saturating_sub(1),
        left(*numbers).min(creator.budget.spend),
        implies,
    );
    if creator.delegation.depth == 0 {
        parts.depth = true;
    }
    if numbers.spent.checked_add(numbers.spent_below).is_none() {
        parts.spend = true;
    }
    parts
}

pub(crate) fn differences(
    a: &Authority,
    b: &Authority,
    tasks: u32,
    depth: u32,
    spend: u64,
    implies: &Implies,
) -> Lacks {
    let deadline_fits = match a.budget.deadline {
        Some(deadline) => match b.budget.deadline {
            Some(ceiling) => deadline <= ceiling,
            None => true,
        },
        None => b.budget.deadline.is_none(),
    };
    let mut lacks = Lacks {
        tools: a.tools.0 & !b.tools.0 != 0,
        grants: false,
        executors: false,
        tasks: a.delegation.tasks > tasks,
        depth: a.delegation.depth > depth,
        spend: a.budget.spend > spend,
        deadline: !deadline_fits,
        notes: a.notes.0 & !b.notes.0 != 0,
    };
    for kind in &a.delegation.kinds {
        if !b.delegation.kinds.contains(kind) {
            lacks.executors = true;
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
            lacks.grants = true;
        }
    }
    lacks
}
