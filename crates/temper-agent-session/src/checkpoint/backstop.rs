// SPDX-License-Identifier: MPL-2.0

//! The mechanical safety-net checkpoint decision.
//!
//! The model drives normal checkpoints via the `checkpoint` tool; the backstop
//! only fires when the lease is about to expire or too long has passed since the
//! last push, so crash-recovery stays bounded.

use std::time::{Duration, SystemTime};

/// How close to the deadline (lease expiry) the backstop fires.
pub(super) const CHECKPOINT_DEADLINE_MARGIN: Duration = Duration::from_secs(60);

/// Pure backstop decision: checkpoint when the deadline is within `margin`
/// (or already passed), or when `interval` has elapsed since the last push.
pub(super) fn backstop_decision(
    now: SystemTime,
    deadline: Option<SystemTime>,
    margin: Duration,
    since_last: Duration,
    interval: Duration,
) -> bool {
    if let Some(deadline) = deadline {
        match deadline.duration_since(now) {
            Ok(remaining) => {
                if remaining <= margin {
                    return true;
                }
            }
            Err(_) => return true,
        }
    }
    since_last >= interval
}

#[cfg(test)]
mod tests {
    use super::backstop_decision;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn backstop_fires_near_deadline_or_after_interval() {
        let now = UNIX_EPOCH + Duration::from_secs(10_000);
        let margin = Duration::from_secs(60);
        let interval = Duration::from_secs(300);
        let recent = Duration::from_secs(10);

        // Deadline comfortably ahead and interval not elapsed: not due (the
        // model drives normal checkpoints).
        assert!(!backstop_decision(
            now,
            Some(now + Duration::from_secs(600)),
            margin,
            recent,
            interval
        ));
        // Deadline within the margin: due.
        assert!(backstop_decision(
            now,
            Some(now + Duration::from_secs(30)),
            margin,
            recent,
            interval
        ));
        // Deadline already passed: due.
        assert!(backstop_decision(
            now,
            Some(now - Duration::from_secs(5)),
            margin,
            Duration::ZERO,
            interval
        ));
        // No deadline, interval elapsed: due (bounded crash-recovery fallback).
        assert!(backstop_decision(
            now,
            None,
            margin,
            Duration::from_secs(301),
            interval
        ));
        // No deadline, interval not elapsed: not due.
        assert!(!backstop_decision(
            now,
            None,
            margin,
            Duration::from_secs(120),
            interval
        ));
    }
}
