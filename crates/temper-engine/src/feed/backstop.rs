// SPDX-License-Identifier: MPL-2.0

//! The poll-backstop: a fixed-delay cadence loop that re-scans configured
//! repository/role targets, re-feeding ready work the webhook edge-trigger may
//! have missed (ADR 0009).

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use temper_engine_io::Spawner;
use temper_forge::{Forge, RepositoryId};
use temper_workflow::{CompiledWorkflow, RoleId, ValidatedWorkflow};

use crate::Daemon;
use crate::lease_applier::WallClock;

/// Which read-only scan the daemon feed runs for a role.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RoleFeedMode {
    /// Steady-state active-queue scan (`scan_role`). This is the default mode.
    #[default]
    Normal,
    /// Wake-triggered scan (`scan_role_wake`).
    Wake,
}

/// A configured repository/role feed target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoleFeedTarget {
    pub repo: RepositoryId,
    pub role: RoleId,
    pub mode: RoleFeedMode,
}

/// Configured target set and fixed delay between poll-backstop passes.
#[derive(Clone, Debug)]
pub struct PollBackstopConfig {
    /// Targets scanned in order on each cadence tick.
    pub targets: Vec<RoleFeedTarget>,
    /// Delay after one complete pass before the next pass starts.
    pub cadence: Duration,
}

/// Runs one poll-backstop pass over the configured targets.
pub async fn run_poll_backstop_tick<F: Forge + ?Sized>(
    daemon: &Daemon,
    forge: &F,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    now: DateTime<Utc>,
    config: &PollBackstopConfig,
) -> usize {
    let mut total = 0;
    for target in &config.targets {
        match daemon
            .enqueue_scanned_role_work(
                forge,
                &target.repo,
                workflow,
                compiled,
                now,
                &target.role,
                target.mode,
            )
            .await
        {
            Ok(count) => total += count,
            Err(error) => tracing::warn!(
                target: "engine",
                repo = %target.repo,
                role = %target.role.as_str(),
                %error,
                "poll backstop scan failed"
            ),
        }
    }
    if total > 0 {
        tracing::debug!(target: "engine", "{}", poll_backstop_log_line(total));
    }
    total
}

fn poll_backstop_log_line(enqueued: usize) -> String {
    format!("engine: poll backstop enqueued={enqueued}")
}

/// Spawns a machine-driven fixed-delay poll backstop onto the engine runtime.
///
/// A cadence machine requests one tick, the shell executes the scan, and the
/// next tick is scheduled one cadence after the previous tick completed.
pub fn spawn_poll_backstop<F: Forge + Send + Sync + ?Sized + 'static>(
    spawner: &Arc<dyn Spawner>,
    daemon: Daemon,
    forge: Arc<F>,
    workflow: Arc<ValidatedWorkflow>,
    compiled: Arc<CompiledWorkflow>,
    config: PollBackstopConfig,
    clock: WallClock,
) {
    let cadence = config.cadence;
    temper_engine_io::spawn_cadence_loop(spawner, cadence, move || {
        let daemon = daemon.clone();
        let forge = Arc::clone(&forge);
        let workflow = Arc::clone(&workflow);
        let compiled = Arc::clone(&compiled);
        let config = config.clone();
        let now = clock();
        async move {
            run_poll_backstop_tick(
                &daemon,
                forge.as_ref(),
                workflow.as_ref(),
                compiled.as_ref(),
                now,
                &config,
            )
            .await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::poll_backstop_log_line;

    #[test]
    fn poll_backstop_log_line_includes_enqueued_count() {
        assert_eq!(
            poll_backstop_log_line(5),
            "engine: poll backstop enqueued=5"
        );
    }
}
