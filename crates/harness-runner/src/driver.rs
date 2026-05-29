//! Single-process worker driver.
//!
//! [`FixpointDriver`] is the deterministic scheduler used by in-process stages
//! and tests. It ticks every worker round-robin until one full pass makes no
//! progress. Later production topologies use a poll loop per process, but the
//! unit of work remains the same [`Worker`](crate::Worker) trait.

use crate::{Worker, WorkerError};
use chrono::{DateTime, Duration, Utc};
use std::error::Error;
use std::fmt;
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

    fn after_tick(&self) {
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

/// One worker's contribution to a fixpoint run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerRunReport {
    /// Worker name as reported by [`Worker::name`].
    pub name: String,
    /// Number of times this worker was ticked.
    pub ticks: u64,
    /// Sum of changed actions reported by this worker.
    pub actions: u64,
}

/// Summary of a driver run.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RunReport {
    /// Total worker ticks performed.
    pub ticks: u64,
    /// Per-worker tick and action counts in driver order.
    pub workers: Vec<WorkerRunReport>,
}

impl RunReport {
    /// Returns the action count for the first worker named `name`.
    pub fn action_count(&self, name: &str) -> Option<u64> {
        self.workers
            .iter()
            .find(|worker| worker.name == name)
            .map(|worker| worker.actions)
    }
}

/// Error returned by a fixpoint run.
#[derive(Debug)]
pub enum DriveError {
    /// A worker tick failed.
    Worker {
        /// Worker that returned the error.
        worker: String,
        /// Underlying worker error.
        source: WorkerError,
    },
    /// The run exhausted its tick budget before reaching a fixpoint.
    NotConverged {
        /// Maximum worker ticks allowed.
        budget: u64,
        /// Partial report accumulated before the budget was exhausted.
        report: RunReport,
    },
}

impl fmt::Display for DriveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DriveError::Worker { worker, source } => {
                write!(formatter, "worker {worker} failed: {source}")
            }
            DriveError::NotConverged { budget, report } => write!(
                formatter,
                "fixpoint driver did not converge within {budget} ticks (ran {})",
                report.ticks
            ),
        }
    }
}

impl Error for DriveError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            DriveError::Worker { source, .. } => Some(source),
            DriveError::NotConverged { .. } => None,
        }
    }
}

/// Deterministic single-process scheduler for a worker set.
pub struct FixpointDriver<'a> {
    workers: Vec<&'a dyn Worker>,
    clock: ManualClock,
}

impl<'a> FixpointDriver<'a> {
    /// Creates a driver with the default fixed clock.
    pub fn new(workers: Vec<&'a dyn Worker>) -> Self {
        Self::with_clock(workers, ManualClock::default())
    }

    /// Creates a driver with an explicit clock.
    pub fn with_clock(workers: Vec<&'a dyn Worker>, clock: ManualClock) -> Self {
        Self { workers, clock }
    }

    /// Returns the clock used by this driver.
    pub fn clock(&self) -> &ManualClock {
        &self.clock
    }

    /// Runs workers round-robin until a full pass makes no progress.
    pub async fn run(&self, budget: u64) -> Result<RunReport, DriveError> {
        let mut report = RunReport {
            ticks: 0,
            workers: self
                .workers
                .iter()
                .map(|worker| WorkerRunReport {
                    name: worker.name().to_string(),
                    ticks: 0,
                    actions: 0,
                })
                .collect(),
        };
        if self.workers.is_empty() {
            return Ok(report);
        }

        loop {
            let mut changed = false;
            for (index, worker) in self.workers.iter().enumerate() {
                if report.ticks >= budget {
                    return Err(DriveError::NotConverged { budget, report });
                }

                let progress =
                    worker
                        .tick(self.clock.now())
                        .await
                        .map_err(|source| DriveError::Worker {
                            worker: worker.name().to_string(),
                            source,
                        })?;
                self.clock.after_tick();
                changed |= progress.changed;
                report.ticks = report.ticks.saturating_add(1);
                report.workers[index].ticks = report.workers[index].ticks.saturating_add(1);
                report.workers[index].actions = report.workers[index]
                    .actions
                    .saturating_add(u64::from(progress.actions));
            }

            if !changed {
                return Ok(report);
            }
        }
    }
}
