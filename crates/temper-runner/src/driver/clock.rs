//! Deterministic manual clock shared with drivers.

use chrono::{DateTime, Duration, Utc};
use std::sync::{Arc, Mutex};

/// Mutable clock shared with a driver.
///
/// The default clock is fixed. Tests that need time to move can call
/// [`advance`](Self::advance), or configure a per-tick step with
/// [`set_tick_step`](Self::set_tick_step).
#[derive(Clone, Debug)]
pub struct ManualClock {
    state: Arc<Mutex<ClockState>>,
}

#[derive(Clone, Debug)]
struct ClockState {
    now: DateTime<Utc>,
    tick_step: Duration,
}

impl ManualClock {
    /// Creates a fixed clock at `now`.
    pub fn fixed(now: DateTime<Utc>) -> Self {
        Self {
            state: Arc::new(Mutex::new(ClockState {
                now,
                tick_step: Duration::zero(),
            })),
        }
    }

    /// Creates a clock that advances by `tick_step` after every worker tick.
    pub fn with_tick_step(now: DateTime<Utc>, tick_step: Duration) -> Self {
        let clock = Self::fixed(now);
        clock.set_tick_step(tick_step);
        clock
    }

    /// Returns the current time.
    pub fn now(&self) -> DateTime<Utc> {
        self.lock().now
    }

    /// Advances the clock immediately by `duration`.
    pub fn advance(&self, duration: Duration) {
        let mut state = self.lock();
        state.now += duration;
    }

    /// Sets the amount of time added after each worker tick.
    pub fn set_tick_step(&self, tick_step: Duration) {
        self.lock().tick_step = tick_step;
    }

    pub(super) fn after_tick(&self) {
        let mut state = self.lock();
        if !state.tick_step.is_zero() {
            let step = state.tick_step;
            state.now += step;
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ClockState> {
        self.state.lock().expect("driver clock mutex is poisoned")
    }
}

impl Default for ManualClock {
    fn default() -> Self {
        Self::fixed(DateTime::<Utc>::from_timestamp(0, 0).expect("Unix epoch is valid"))
    }
}
