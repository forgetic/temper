// SPDX-License-Identifier: MPL-2.0

//! Mechanical backstop loop owned by the daemon process.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use temper_forge::{ChangeHint, Forge, ForgeError};
use temper_runner::{
    MultiRepoMechanicalWorker, MultiRepoTickReport, Progress, RepositoryJournal, RepositorySet,
    WorkerError,
};
use temper_workflow::{InMemoryJournal, LeasePolicy, ValidatedWorkflow};

/// Configuration for the daemon's mechanical backstop loop.
#[derive(Clone, Debug)]
pub struct MechanicalBackstopConfig {
    /// Repositories ticked in order on each pass.
    pub repositories: RepositorySet,
    /// Delay after one complete pass before the next pass starts.
    pub cadence: Duration,
    /// Lease policy for mechanical transitions (reuse the daemon's lease TTL).
    pub lease_policy: LeasePolicy,
}

/// Which repositories a mechanical pass covers.
#[derive(Clone, Debug, Default)]
pub enum MechanicalScope {
    /// All configured repositories — the level-triggered liveness backstop.
    #[default]
    All,
    /// Only repositories named by these webhook change hints — the
    /// edge-triggered accelerator. A hint for an unconfigured repo simply
    /// matches nothing (the pass is a no-op), so a forged/stale hint is safe.
    Hinted(Vec<ChangeHint>),
}

/// Runs one mechanical pass over the in-scope repositories.
///
/// The worker is constructed per call so tests can tick deterministically while
/// the caller owns the per-repository journals across calls. Workspace-backed
/// automations intentionally run with no external tool executors bound; those
/// automations no-op safely until a binding exists.
///
/// `scope` selects coverage: [`MechanicalScope::All`] is the slow backstop pass;
/// [`MechanicalScope::Hinted`] ticks only the hinted repositories, which is how a
/// webhook accelerates the mechanical loop without paying for every repo (ADR
/// 0009: webhooks are the edge-triggered accelerator, polling the backstop).
pub async fn run_mechanical_backstop_tick<F: Forge + ?Sized>(
    forge: &F,
    workflow: &ValidatedWorkflow,
    now: DateTime<Utc>,
    config: &MechanicalBackstopConfig,
    journals: &[InMemoryJournal],
    scope: &MechanicalScope,
) -> Result<Progress, WorkerError> {
    if journals.len() != config.repositories.repositories().len() {
        let error = setup_error(format!(
            "mechanical backstop has {} repositories but {} journals",
            config.repositories.repositories().len(),
            journals.len()
        ));
        eprintln!("temper-daemon: mechanical backstop tick failed: {error}");
        return Err(error);
    }

    let journal_bindings: Vec<RepositoryJournal<'_, InMemoryJournal>> = config
        .repositories
        .repositories()
        .iter()
        .zip(journals.iter())
        .map(|(repository, journal)| RepositoryJournal {
            repository: &repository.id,
            journal,
        })
        .collect();
    let worker = match MultiRepoMechanicalWorker::new(
        workflow,
        forge,
        config.repositories.clone(),
        journal_bindings,
        config.lease_policy,
    ) {
        Ok(worker) => worker,
        Err(error) => {
            let error = setup_error(format!("mechanical backstop setup failed: {error}"));
            eprintln!("temper-daemon: mechanical backstop tick failed: {error}");
            return Err(error);
        }
    };

    let report: MultiRepoTickReport = match scope {
        MechanicalScope::All => worker.tick_report(now).await,
        MechanicalScope::Hinted(hints) => worker.tick_matching_hints(now, hints).await,
    };
    match report.into_worker_result() {
        Ok(progress) => Ok(progress),
        Err(error) => {
            eprintln!("temper-daemon: mechanical backstop tick failed: {error}");
            Err(error)
        }
    }
}

/// Owns the per-repo journals and forge/workflow handles, and runs mechanical
/// passes — either the full backstop (all repos) or a webhook-hinted pass (only
/// the named repos). Cloneable and `Send + Sync` so the cadence loop and the
/// webhook scanner share one set of journals.
///
/// Passes are **coalesced**: while one pass is running, a concurrently requested
/// pass is skipped rather than interleaved. The in-flight (or next backstop)
/// pass re-reads fresh forge state and so subsumes the skipped one — matching the
/// ADR 0009 "coalesce bursts" guidance and keeping the per-repo journals free of
/// concurrent mutation on the single-threaded engine.
pub struct MechanicalTrigger<F: Forge + Send + Sync + 'static> {
    forge: Arc<F>,
    workflow: Arc<ValidatedWorkflow>,
    config: MechanicalBackstopConfig,
    journals: Arc<Vec<InMemoryJournal>>,
    clock: crate::WallClock,
    /// Coalescing guard: `true` while a pass is in flight. A requested pass that
    /// finds it already set returns immediately (the running pass covers it).
    running: Arc<std::sync::Mutex<bool>>,
}

impl<F: Forge + Send + Sync + 'static> Clone for MechanicalTrigger<F> {
    fn clone(&self) -> Self {
        Self {
            forge: Arc::clone(&self.forge),
            workflow: Arc::clone(&self.workflow),
            config: self.config.clone(),
            journals: Arc::clone(&self.journals),
            clock: self.clock.clone(),
            running: Arc::clone(&self.running),
        }
    }
}

impl<F: Forge + Send + Sync + 'static> MechanicalTrigger<F> {
    /// Builds a trigger with one fresh in-memory journal per configured repo.
    pub fn new(
        forge: Arc<F>,
        workflow: Arc<ValidatedWorkflow>,
        config: MechanicalBackstopConfig,
        clock: crate::WallClock,
    ) -> Self {
        let journals = Arc::new(
            config
                .repositories
                .repositories()
                .iter()
                .map(|_| InMemoryJournal::new())
                .collect(),
        );
        Self {
            forge,
            workflow,
            config,
            journals,
            clock,
            running: Arc::new(std::sync::Mutex::new(false)),
        }
    }

    /// Runs one pass over `scope`, coalescing against any in-flight pass.
    ///
    /// Returns `false` without running when a pass is already in flight (the
    /// running pass subsumes this request); otherwise runs the pass and returns
    /// `true`. Tick errors are logged inside [`run_mechanical_backstop_tick`].
    pub async fn run(&self, scope: MechanicalScope) -> bool {
        // Claim the guard; bail if a pass is already running. The lock is held
        // only for the flag flip, never across the await.
        {
            let mut running = self
                .running
                .lock()
                .expect("mechanical running flag poisoned");
            if *running {
                return false;
            }
            *running = true;
        }
        let now = (self.clock)();
        let _ = run_mechanical_backstop_tick(
            self.forge.as_ref(),
            self.workflow.as_ref(),
            now,
            &self.config,
            &self.journals,
            &scope,
        )
        .await;
        *self
            .running
            .lock()
            .expect("mechanical running flag poisoned") = false;
        true
    }

    /// Accelerator entry point: run a pass covering only the hinted repositories.
    /// Called from the webhook path so an event triggers immediate mechanical
    /// reconciliation for the affected repo (ADR 0009 edge-trigger).
    pub async fn run_hinted(&self, hints: Vec<ChangeHint>) -> bool {
        if hints.is_empty() {
            return false;
        }
        self.run(MechanicalScope::Hinted(hints)).await
    }
}

/// Owns the per-repo journals and runs the mechanical backstop until the
/// runtime shuts down, as a machine-driven cadence loop on the engine. Returns a
/// [`MechanicalTrigger`] sharing the same journals so a webhook can run an
/// immediate hinted pass between the (slow) backstop ticks.
///
/// Tick errors are logged by [`run_mechanical_backstop_tick`] and skipped so a
/// transient backend failure does not stop the daemon-owned backstop.
pub fn spawn_mechanical_backstop<F: Forge + Send + Sync + 'static>(
    spawner: &std::sync::Arc<dyn temper_engine_io::Spawner>,
    forge: std::sync::Arc<F>,
    workflow: std::sync::Arc<ValidatedWorkflow>,
    config: MechanicalBackstopConfig,
    clock: crate::WallClock,
) -> MechanicalTrigger<F> {
    let cadence = config.cadence;
    let trigger = MechanicalTrigger::new(forge, workflow, config, clock);
    let loop_trigger = trigger.clone();
    temper_engine_io::spawn_cadence_loop(spawner, cadence, move || {
        let trigger = loop_trigger.clone();
        async move {
            trigger.run(MechanicalScope::All).await;
        }
    });
    trigger
}

fn setup_error(message: String) -> WorkerError {
    WorkerError::Forge(ForgeError::Backend(message))
}
