//! Deterministic single-process round-robin scheduler.

use super::clock::ManualClock;
use super::report::{DriveError, RunReport, WorkerRunReport};
use crate::Worker;

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
