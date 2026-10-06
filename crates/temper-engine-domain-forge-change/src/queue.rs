//! A derived landing queue (domain/forge.md, section 9).
//!
//! No queue is stored. The top supplies ready changes for one branch;
//! `first` chooses one by age, then priority and first-ready time.
use crate::Ready;
use skein_lib::{Duration, Wall};

/// Choose the first ready change, with task number as a stable final tie.
#[must_use]
#[expect(clippy::manual_map, reason = "the strict subset forbids closures")]
pub fn first(ready: &[Ready], window: Duration, now: Wall) -> Option<u64> {
    let mut selected: Option<Ready> = None;
    for candidate in ready {
        selected = match selected {
            Some(current) if before(&current, candidate, window, now) => Some(current),
            Some(_) | None => Some(*candidate),
        };
    }
    match selected {
        Some(chosen) => Some(chosen.task),
        None => None,
    }
}

fn before(a: &Ready, b: &Ready, window: Duration, now: Wall) -> bool {
    let a_aged = now.as_nanos().saturating_sub(a.since.as_nanos()) >= window.as_nanos();
    let b_aged = now.as_nanos().saturating_sub(b.since.as_nanos()) >= window.as_nanos();
    if a_aged != b_aged {
        return a_aged;
    }
    if a_aged && a.since != b.since {
        return a.since < b.since;
    }
    if a.priority != b.priority {
        return a.priority > b.priority;
    }
    if a.since != b.since {
        return a.since < b.since;
    }
    a.task < b.task
}
