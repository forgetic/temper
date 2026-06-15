//! Driver run reports and errors.

use crate::WorkerError;
use std::error::Error;
use std::fmt;

/// One worker's contribution to a fixpoint run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerRunReport {
    /// Worker name as reported by [`Worker::name`](crate::Worker::name).
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
