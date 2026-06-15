//! Worker primitives for driving runner progress.
//!
//! A [`Worker`] is the unit a driver ticks. [`RoleWorker`] is the per-role
//! production worker: every tick scans fresh Forge state for that role and lets
//! the role's [`Agent`](crate::Agent) service each active
//! [`WorkItem`](crate::WorkItem) through [`RoleTools`](crate::RoleTools).
//! [`MechanicalWorker`] is the controller-plane worker: every normal tick runs
//! bounded reconciliation/recovery and then services declared automated queues
//! through workflow transitions, so mechanical state changes converge without
//! spawning an agent.

mod automation;
mod error;
mod mechanical;
mod role;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

pub use error::WorkerError;
pub use mechanical::MechanicalWorker;
pub use role::RoleWorker;

/// Progress made by one worker tick.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub struct Progress {
    /// Whether the tick changed workflow state.
    pub changed: bool,
    /// Number of workflow-state changes this tick carried through.
    pub actions: u32,
}

impl Progress {
    /// A tick with no changes.
    pub fn unchanged() -> Self {
        Self::default()
    }

    /// Records one service result.
    pub fn record(&mut self, changed: bool) {
        if changed {
            self.changed = true;
            self.actions = self.actions.saturating_add(1);
        }
    }
}

/// Tickable runner unit.
#[async_trait]
pub trait Worker: Send + Sync {
    /// Advances this worker once at `now`.
    async fn tick(&self, now: DateTime<Utc>) -> Result<Progress, WorkerError>;

    /// Stable human-readable worker name.
    fn name(&self) -> &str;
}

pub(crate) fn saturating_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

pub(crate) fn saturating_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}
