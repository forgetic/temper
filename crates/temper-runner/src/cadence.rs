//! Poll-cadence helpers shared by production and testing drivers.

use crate::worker::Progress;
use std::time::Duration;

/// Exponential backoff for repeated successful no-action mechanical poll ticks.
///
/// The first idle tick keeps the active poll interval, then each additional
/// consecutive idle tick doubles the delay until `max_interval`. Any progress,
/// wake, audit, or error should call [`reset`](Self::reset) so the next poll uses
/// the active low-latency interval again.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdlePollBackoff {
    active_interval: Duration,
    max_interval: Duration,
    consecutive_idle_ticks: u32,
}

impl IdlePollBackoff {
    /// Creates a backoff policy. `max_interval` is clamped to at least
    /// `active_interval` so enabling the policy can never poll faster than the
    /// configured active cadence.
    pub fn new(active_interval: Duration, max_interval: Duration) -> Self {
        let active_interval = if active_interval.is_zero() {
            Duration::from_millis(1)
        } else {
            active_interval
        };
        let max_interval = if max_interval < active_interval {
            active_interval
        } else {
            max_interval
        };
        Self {
            active_interval,
            max_interval,
            consecutive_idle_ticks: 0,
        }
    }

    /// Configured low-latency poll interval used while active or after reset.
    pub fn active_interval(&self) -> Duration {
        self.active_interval
    }

    /// Maximum poll interval reached after repeated idle ticks.
    pub fn max_interval(&self) -> Duration {
        self.max_interval
    }

    /// Current consecutive successful no-action normal tick count.
    pub fn consecutive_idle_ticks(&self) -> u32 {
        self.consecutive_idle_ticks
    }

    /// Records a successful normal mechanical tick and returns the next poll delay.
    pub fn record_normal_tick(&mut self, progress: Progress) -> Duration {
        if progress.changed || progress.actions > 0 {
            self.reset()
        } else {
            self.consecutive_idle_ticks = self.consecutive_idle_ticks.saturating_add(1);
            self.current_interval()
        }
    }

    /// Resets backoff and returns the active poll delay.
    pub fn reset(&mut self) -> Duration {
        self.consecutive_idle_ticks = 0;
        self.active_interval
    }

    /// Returns the delay represented by the current idle counter.
    pub fn current_interval(&self) -> Duration {
        let mut delay = self.active_interval;
        let mut remaining_doubles = self.consecutive_idle_ticks.saturating_sub(1);
        while remaining_doubles > 0 && delay < self.max_interval {
            delay = delay.saturating_add(delay);
            remaining_doubles -= 1;
        }
        if delay > self.max_interval {
            self.max_interval
        } else {
            delay
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_no_action_ticks_stretch_then_cap() {
        let mut backoff = IdlePollBackoff::new(Duration::from_secs(1), Duration::from_secs(8));

        assert_eq!(
            backoff.record_normal_tick(Progress::unchanged()),
            Duration::from_secs(1)
        );
        assert_eq!(
            backoff.record_normal_tick(Progress::unchanged()),
            Duration::from_secs(2)
        );
        assert_eq!(
            backoff.record_normal_tick(Progress::unchanged()),
            Duration::from_secs(4)
        );
        assert_eq!(
            backoff.record_normal_tick(Progress::unchanged()),
            Duration::from_secs(8)
        );
        assert_eq!(
            backoff.record_normal_tick(Progress::unchanged()),
            Duration::from_secs(8)
        );
        assert_eq!(backoff.consecutive_idle_ticks(), 5);
    }

    #[test]
    fn progress_and_reset_restore_active_cadence() {
        let mut backoff = IdlePollBackoff::new(Duration::from_secs(1), Duration::from_secs(8));
        backoff.record_normal_tick(Progress::unchanged());
        backoff.record_normal_tick(Progress::unchanged());
        backoff.record_normal_tick(Progress::unchanged());
        assert_eq!(backoff.current_interval(), Duration::from_secs(4));

        assert_eq!(
            backoff.record_normal_tick(Progress {
                changed: false,
                actions: 1,
            }),
            Duration::from_secs(1)
        );
        assert_eq!(backoff.consecutive_idle_ticks(), 0);

        backoff.record_normal_tick(Progress::unchanged());
        backoff.record_normal_tick(Progress::unchanged());
        assert_eq!(backoff.reset(), Duration::from_secs(1));
        assert_eq!(backoff.consecutive_idle_ticks(), 0);
    }

    #[test]
    fn max_interval_never_shortens_active_interval() {
        let mut backoff = IdlePollBackoff::new(Duration::from_secs(30), Duration::from_secs(5));

        assert_eq!(backoff.active_interval(), Duration::from_secs(30));
        assert_eq!(backoff.max_interval(), Duration::from_secs(30));
        assert_eq!(
            backoff.record_normal_tick(Progress::unchanged()),
            Duration::from_secs(30)
        );
        assert_eq!(
            backoff.record_normal_tick(Progress::unchanged()),
            Duration::from_secs(30)
        );
    }
}
