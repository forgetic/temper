//! Feed the model events, inspect the requests that come out.

use temper_lib::Duration;

use crate::{Limits, worst_case};

const LIMITS: Limits = Limits {
    workers: 3,
    slots: 2,
    workstreams: 2,
    workstream_bytes: 8,
    attempts: 6,
    calls: 3,
    grace: Duration::from_secs(10),
    facts: 64,
};

#[test]
fn the_worst_case_is_bounded_or_refused() {
    assert!(worst_case(&LIMITS).is_some());
    let huge = Limits { workers: u32::MAX, slots: u32::MAX, workstream_bytes: u32::MAX, ..LIMITS };
    assert_eq!(worst_case(&huge), None);
}
