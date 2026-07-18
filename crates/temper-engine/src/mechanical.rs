// SPDX-License-Identifier: MPL-2.0

//! Mechanical backstop loop owned by the daemon process.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use temper_forge::{ChangeHint, Forge, ForgeError};
use temper_runner::{
    ArtifactAddress, MultiRepoMechanicalWorker, MultiRepoTickReport, Progress,
    PullRequestMergeObserver, RepositoryJournal, RepositorySet, WorkerError,
};
use temper_workflow::{
    InMemoryJournal, LeasePolicy, ReconciliationDetailCache, ReconciliationDetailCachePolicy,
    ValidatedWorkflow,
};

/// Configuration for the daemon's mechanical backstop loop.
#[derive(Clone)]
pub struct MechanicalBackstopConfig {
    /// Repositories ticked in order on each pass.
    pub repositories: RepositorySet,
    /// Delay after one complete pass before the next pass starts.
    pub cadence: Duration,
    /// Lease policy for mechanical transitions (reuse the daemon's lease TTL).
    pub lease_policy: LeasePolicy,
    /// Worker/standalone-owned hook for filesystem cleanup after a landed PR.
    /// Split deployments leave this unbound until a worker protocol cleanup
    /// message exists.
    pub pull_request_merge_observer: Option<Arc<dyn PullRequestMergeObserver>>,
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
    /// Exact artifact work admitted by the daemon coordinator. Unlike hinted
    /// broad work this uses exact fetch and bounded targeted reconciliation.
    Targeted(
        Vec<(
            temper_forge::RepositoryPath,
            ArtifactAddress,
            temper_forge::ChangeKind,
        )>,
    ),
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
    let cache = ReconciliationDetailCache::default();
    run_mechanical_backstop_tick_with_cache(forge, workflow, now, config, journals, scope, cache)
        .await
}

async fn run_mechanical_backstop_tick_with_cache<F: Forge + ?Sized>(
    forge: &F,
    workflow: &ValidatedWorkflow,
    now: DateTime<Utc>,
    config: &MechanicalBackstopConfig,
    journals: &[InMemoryJournal],
    scope: &MechanicalScope,
    cache: ReconciliationDetailCache,
) -> Result<Progress, WorkerError> {
    if journals.len() != config.repositories.repositories().len() {
        let error = setup_error(format!(
            "mechanical backstop has {} repositories but {} journals",
            config.repositories.repositories().len(),
            journals.len()
        ));
        tracing::warn!(target: "temper_daemon", %error, "mechanical backstop tick failed");
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
    let mut worker = match MultiRepoMechanicalWorker::new(
        workflow,
        forge,
        config.repositories.clone(),
        journal_bindings,
        config.lease_policy,
    ) {
        Ok(worker) => worker.with_reconciliation_detail_cache(cache),
        Err(error) => {
            let error = setup_error(format!("mechanical backstop setup failed: {error}"));
            tracing::warn!(target: "temper_daemon", %error, "mechanical backstop tick failed");
            return Err(error);
        }
    };
    if let Some(observer) = &config.pull_request_merge_observer {
        worker = worker.with_pull_request_merge_observer(Arc::clone(observer));
    }

    let report: MultiRepoTickReport = match scope {
        MechanicalScope::All => worker.tick_report(now).await,
        MechanicalScope::Hinted(hints) => worker.tick_matching_hints(now, hints).await,
        MechanicalScope::Targeted(targets) => worker.tick_targeted(now, targets).await,
    };
    match report.into_worker_result() {
        Ok(progress) => Ok(progress),
        Err(error) => {
            tracing::warn!(target: "temper_daemon", %error, "mechanical backstop tick failed");
            Err(error)
        }
    }
}

/// Owns the per-repository journals, bounded reconciliation detail cache, and
/// forge/workflow handles used to execute mechanical work already admitted by
/// the daemon `WakeCoordinator`.
///
/// This type intentionally has no admission or coalescing state. The daemon
/// coordinator is the sole production owner of pending, in-flight, dirty, and
/// apply-deferred work, so every admitted call is executed rather than skipped
/// behind a second boolean guard.
pub struct MechanicalTrigger<F: Forge + Send + Sync + ?Sized + 'static> {
    forge: Arc<F>,
    workflow: Arc<ValidatedWorkflow>,
    config: MechanicalBackstopConfig,
    journals: Arc<Vec<InMemoryJournal>>,
    reconciliation_detail_cache: ReconciliationDetailCache,
    clock: crate::WallClock,
}

impl<F: Forge + Send + Sync + ?Sized + 'static> Clone for MechanicalTrigger<F> {
    fn clone(&self) -> Self {
        Self {
            forge: Arc::clone(&self.forge),
            workflow: Arc::clone(&self.workflow),
            config: self.config.clone(),
            journals: Arc::clone(&self.journals),
            reconciliation_detail_cache: self.reconciliation_detail_cache.clone(),
            clock: self.clock.clone(),
        }
    }
}

impl<F: Forge + Send + Sync + ?Sized + 'static> MechanicalTrigger<F> {
    /// Builds a trigger with one fresh in-memory journal per configured repo
    /// and the production reconciliation detail-cache policy.
    pub fn new(
        forge: Arc<F>,
        workflow: Arc<ValidatedWorkflow>,
        config: MechanicalBackstopConfig,
        clock: crate::WallClock,
    ) -> Self {
        Self::new_with_reconciliation_cache_policy(
            forge,
            workflow,
            config,
            clock,
            ReconciliationDetailCachePolicy::default(),
        )
    }

    /// Builds a trigger with an injectable reconciliation detail-cache policy.
    pub fn new_with_reconciliation_cache_policy(
        forge: Arc<F>,
        workflow: Arc<ValidatedWorkflow>,
        config: MechanicalBackstopConfig,
        clock: crate::WallClock,
        cache_policy: ReconciliationDetailCachePolicy,
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
            reconciliation_detail_cache: ReconciliationDetailCache::new(cache_policy),
            clock,
        }
    }

    /// Returns a shared handle for cache observability and explicit runtime
    /// invalidation wiring.
    pub fn reconciliation_detail_cache(&self) -> ReconciliationDetailCache {
        self.reconciliation_detail_cache.clone()
    }

    /// Runs one pass admitted by the daemon coordinator.
    ///
    /// The coordinator already owns pending/in-flight/dirty semantics,
    /// including the mandatory dirty follow-up for hints received after a
    /// generation starts.
    pub(crate) async fn run_coordinated(
        &self,
        scope: MechanicalScope,
    ) -> Result<Progress, WorkerError> {
        let now = (self.clock)();
        run_mechanical_backstop_tick_with_cache(
            self.forge.as_ref(),
            self.workflow.as_ref(),
            now,
            &self.config,
            &self.journals,
            &scope,
            self.reconciliation_detail_cache.clone(),
        )
        .await
    }
}

/// Spawns the production mechanical cadence as bounded daemon wake requests.
/// The cadence callback performs no reconciliation itself.
pub fn spawn_coordinated_mechanical_backstop(
    spawner: &std::sync::Arc<dyn temper_engine_io::Spawner>,
    daemon: crate::Daemon,
    repositories: RepositorySet,
    cadence: Duration,
) {
    let paths = repositories
        .repositories()
        .iter()
        .map(|repository| repository.path.clone())
        .collect::<Vec<_>>();
    temper_engine_io::spawn_cadence_loop(spawner, cadence, move || {
        let daemon = daemon.clone();
        let paths = paths.clone();
        async move {
            for path in paths {
                daemon.schedule_mechanical_poll(path);
            }
        }
    });
}

fn setup_error(message: String) -> WorkerError {
    WorkerError::Forge(ForgeError::Backend(message))
}
