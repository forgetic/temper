//! Controller-plane worker that runs mechanical recovery and automation.

mod observability;

use observability::{
    log_mechanical_reconciliation, log_mechanical_reconciliation_summary, measure_mechanical_phase,
};

use super::automation;
use super::{Progress, Worker, WorkerError, saturating_u32, saturating_u64};
use crate::coding_workspace::ExternalToolExecutors;
use crate::scan::{ArtifactAddress, TargetedArtifactSnapshot, load_targeted_artifact};
use crate::worker::PullRequestMergeObserver;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use temper_forge::{ChangeKind, Forge, ForgeError, HintArtifactKind, ItemNumber, RepositoryId};
use temper_workflow::{
    Applier, ApplyOutcome, ArtifactSnapshot, AssignmentConverger, CompiledWorkflow,
    DefaultRecoveryPolicy, Executor, LeaseManager, LeasePolicy, ReconciliationDetailCache,
    ReconciliationMode, RecoveryAction, RecoveryPolicy, ValidatedWorkflow, parse_metadata_block,
};

/// Controller-plane worker that runs mechanical recovery and automation.
///
/// The worker owns the reusable runtime components for the process — an
/// [`Executor`] and [`LeaseManager`] bound to `forge` — and borrows the process's
/// [`CommandJournal`](temper_workflow::CommandJournal). Normal ticks run bounded
/// reconciliation/apply before declared automated queues. `Escalate` and
/// `Diagnose` actions are not mutations; they are counted by
/// [`advisory_actions`](Self::advisory_actions) so operators or tests can
/// observe them separately from workflow-state changes.
pub struct MechanicalWorker<
    'a,
    F: Forge + ?Sized,
    J: temper_workflow::CommandJournal,
    P: RecoveryPolicy + Send + Sync = DefaultRecoveryPolicy,
> {
    name: String,
    workflow: &'a ValidatedWorkflow,
    compiled: CompiledWorkflow,
    forge: &'a F,
    repo: &'a RepositoryId,
    executor: Executor<'a, F>,
    lease_manager: LeaseManager<'a, F>,
    assignment_converger: AssignmentConverger<'a, F>,
    journal: &'a J,
    policy: P,
    reconciliation_detail_cache: ReconciliationDetailCache,
    /// Workspace executors the actor roles of workspace-backed automations can
    /// invoke. Empty when no executor is bound; such automations no-op until a
    /// binding exists, never failing the tick.
    external_tool_executors: ExternalToolExecutors,
    pull_request_merge_observer: Option<Arc<dyn PullRequestMergeObserver>>,
    advisory_actions: AtomicU64,
}

impl<'a, F, J> MechanicalWorker<'a, F, J, DefaultRecoveryPolicy>
where
    F: Forge + ?Sized,
    J: temper_workflow::CommandJournal,
{
    /// Creates a mechanical worker using [`DefaultRecoveryPolicy`].
    pub fn new(
        workflow: &'a ValidatedWorkflow,
        forge: &'a F,
        repo: &'a RepositoryId,
        journal: &'a J,
        lease_policy: LeasePolicy,
    ) -> Self {
        Self::with_policy(
            workflow,
            forge,
            repo,
            journal,
            lease_policy,
            DefaultRecoveryPolicy,
        )
    }
}

impl<'a, F, J, P> MechanicalWorker<'a, F, J, P>
where
    F: Forge + ?Sized,
    J: temper_workflow::CommandJournal,
    P: RecoveryPolicy + Send + Sync,
{
    /// Creates a mechanical worker with an injectable recovery policy.
    pub fn with_policy(
        workflow: &'a ValidatedWorkflow,
        forge: &'a F,
        repo: &'a RepositoryId,
        journal: &'a J,
        lease_policy: LeasePolicy,
        policy: P,
    ) -> Self {
        Self {
            name: "mechanical".to_string(),
            workflow,
            compiled: workflow.compile(),
            forge,
            repo,
            executor: Executor::new(workflow, forge),
            lease_manager: LeaseManager::new(forge, lease_policy),
            assignment_converger: AssignmentConverger::new(workflow, forge, lease_policy),
            journal,
            policy,
            reconciliation_detail_cache: ReconciliationDetailCache::default(),
            external_tool_executors: ExternalToolExecutors::new(),
            pull_request_merge_observer: None,
            advisory_actions: AtomicU64::new(0),
        }
    }

    /// Binds dependency detail state owned by a longer-lived runtime. Clones of
    /// the cache share the same bounded entries.
    pub fn with_reconciliation_detail_cache(mut self, cache: ReconciliationDetailCache) -> Self {
        self.reconciliation_detail_cache = cache;
        self
    }

    /// Binds workspace executors this worker can invoke from workspace-backed
    /// queue automations. Without this the worker runs only no-executor
    /// automations; workspace-backed ones no-op until a binding exists.
    pub fn with_external_tool_executors(mut self, executors: ExternalToolExecutors) -> Self {
        self.external_tool_executors = executors;
        self
    }

    /// Binds an observer that is notified after this worker observes a pull
    /// request merge.
    pub fn with_pull_request_merge_observer(
        mut self,
        observer: Arc<dyn PullRequestMergeObserver>,
    ) -> Self {
        self.pull_request_merge_observer = Some(observer);
        self
    }

    /// Number of advisory recovery actions observed across ticks.
    pub fn advisory_actions(&self) -> u64 {
        self.advisory_actions.load(Ordering::Relaxed)
    }

    /// Command journal this worker reconciles and updates.
    pub fn journal(&self) -> &J {
        self.journal
    }

    /// Repository path configured for this single-repository worker.
    pub async fn repository_path(&self) -> Result<temper_forge::RepositoryPath, WorkerError> {
        let repository = self
            .forge
            .get_repository(self.repo)
            .await?
            .ok_or_else(|| ForgeError::NotFound(format!("repository {}", self.repo)))?;
        Ok(temper_forge::RepositoryPath::new(
            repository.owner,
            repository.name,
        ))
    }

    /// Evaluates one changed artifact using exact fetch-by-number APIs.
    pub async fn tick_artifact(
        &self,
        now: DateTime<Utc>,
        item: ItemNumber,
        artifact_kind: HintArtifactKind,
        change: ChangeKind,
    ) -> Result<Progress, WorkerError> {
        let address = ArtifactAddress::new(artifact_kind, item);
        let targeted = measure_mechanical_phase(
            self.forge,
            self.repo,
            "targeted",
            Some(address),
            "automated_scan",
            async {
                if change == ChangeKind::Dependency {
                    self.reconciliation_detail_cache
                        .invalidate(self.repo, address.source());
                }
                let mut targeted_snapshots = Vec::new();
                let Some(loaded) =
                    load_targeted_artifact(self.forge, self.repo, self.workflow, address).await?
                else {
                    return Ok(None);
                };
                match &loaded.snapshot {
                    TargetedArtifactSnapshot::Issue(issue) => {
                        self.reconciliation_detail_cache
                            .store_issue(self.repo, issue, now);
                    }
                    TargetedArtifactSnapshot::PullRequest(pull_request) => {
                        self.reconciliation_detail_cache.store_pull_request(
                            self.repo,
                            pull_request,
                            now,
                        );
                    }
                }
                // Staged fan-out children are not externally dispatchable yet.
                // This guard must precede both targeted automation and
                // reconciliation so a webhook cannot mutate a partially wired
                // child.
                if loaded.classified.metadata.staged {
                    return Ok(None);
                }
                match &loaded.snapshot {
                    TargetedArtifactSnapshot::Issue(issue) => {
                        targeted_snapshots.push(ArtifactSnapshot::from_issue(issue));
                    }
                    TargetedArtifactSnapshot::PullRequest(pull_request) => {
                        targeted_snapshots.push(ArtifactSnapshot::from_pull_request(pull_request));
                        if change != ChangeKind::Ci {
                            if let Some(metadata) = parse_metadata_block(&pull_request.body)
                                .map_err(|error| ForgeError::Backend(error.to_string()))?
                            {
                                for parent in metadata
                                    .parents
                                    .iter()
                                    .filter(|parent| parent.is_in_repository(self.repo))
                                {
                                    if let Some(issue) = self
                                        .forge
                                        .get_issue_by_number(self.repo, parent.number)
                                        .await?
                                    {
                                        targeted_snapshots
                                            .push(ArtifactSnapshot::from_issue(&issue));
                                    }
                                }
                            }
                        }
                    }
                }
                let automation_items = crate::scan::targeted_automated_work_items(
                    self.forge,
                    self.repo,
                    self.workflow,
                    &self.compiled,
                    &loaded,
                    now,
                )
                .await?;
                Ok::<_, WorkerError>(Some((targeted_snapshots, automation_items)))
            },
        )
        .await?;
        let Some((targeted_snapshots, automation_items)) = targeted else {
            return Ok(Progress::unchanged());
        };

        let automation_progress = measure_mechanical_phase(
            self.forge,
            self.repo,
            "targeted",
            Some(address),
            "transition_application",
            automation::execute_automated_items(
                &self.name,
                self.repo,
                self.workflow,
                &self.compiled,
                &self.executor,
                &self.external_tool_executors,
                self.forge,
                automation_items,
                self.pull_request_merge_observer.as_ref(),
            ),
        )
        .await?;
        if automation_progress.changed {
            self.reconciliation_detail_cache
                .invalidate_repository(self.repo);
        }
        let reconciliation_progress = measure_mechanical_phase(
            self.forge,
            self.repo,
            "targeted",
            Some(address),
            "reconciliation",
            async {
                if targeted_snapshots.is_empty() {
                    Ok(Progress::unchanged())
                } else {
                    self.reconcile_targeted_snapshots(now, targeted_snapshots)
                        .await
                }
            },
        )
        .await?;
        Ok(Progress {
            changed: automation_progress.changed || reconciliation_progress.changed,
            actions: automation_progress
                .actions
                .saturating_add(reconciliation_progress.actions),
        })
    }

    /// Runs bounded reconciliation using exactly the supplied artifact snapshots.
    pub async fn reconcile_targeted_snapshots(
        &self,
        now: DateTime<Utc>,
        snapshots: Vec<temper_workflow::ArtifactSnapshot>,
    ) -> Result<Progress, WorkerError> {
        let reconciler = self.workflow.reconciler(&self.policy);
        let mut report = reconciler
            .reconcile_bounded(self.forge, self.repo, self.journal, snapshots, now)
            .await?;
        let outcome = if report.is_clean() {
            ApplyOutcome::default()
        } else {
            log_mechanical_reconciliation(&self.name, self.repo, &report);
            Applier::new(&self.executor, &self.lease_manager, self.journal)
                .apply_report_with_assignment_converger(
                    self.repo,
                    &report,
                    now,
                    &self.assignment_converger,
                )
                .await?
        };
        report
            .cache_stats
            .add_invalidations(invalidate_applied_actions(
                &self.reconciliation_detail_cache,
                self.repo,
                &outcome,
            ));
        Ok(Progress {
            changed: !outcome.applied.is_empty(),
            actions: saturating_u32(outcome.applied.len()),
        })
    }

    /// Runs the explicit all-history deep audit path once.
    pub async fn tick_deep_audit(&self, now: DateTime<Utc>) -> Result<Progress, WorkerError> {
        self.tick_with_reconciliation_mode(now, ReconciliationMode::DeepAudit)
            .await
    }

    async fn tick_with_reconciliation_mode(
        &self,
        now: DateTime<Utc>,
        mode: ReconciliationMode,
    ) -> Result<Progress, WorkerError> {
        let reconciliation_progress = measure_mechanical_phase(
            self.forge,
            self.repo,
            "broad",
            None,
            "reconciliation",
            async {
                let reconciler = self.workflow.reconciler(&self.policy);
                let mut report = match mode {
                    ReconciliationMode::Bounded => {
                        reconciler
                            .reconcile_with_detail_cache(
                                self.forge,
                                self.repo,
                                self.journal,
                                now,
                                &self.reconciliation_detail_cache,
                            )
                            .await?
                    }
                    ReconciliationMode::DeepAudit => {
                        let invalidations = self
                            .reconciliation_detail_cache
                            .invalidate_repository(self.repo);
                        let mut report = reconciler
                            .reconcile_deep_audit(self.forge, self.repo, self.journal, now)
                            .await?;
                        report.cache_stats.add_invalidations(invalidations);
                        report
                    }
                };
                let outcome = if report.is_clean() {
                    ApplyOutcome::default()
                } else {
                    log_mechanical_reconciliation(&self.name, self.repo, &report);
                    Applier::new(&self.executor, &self.lease_manager, self.journal)
                        .apply_report_with_assignment_converger(
                            self.repo,
                            &report,
                            now,
                            &self.assignment_converger,
                        )
                        .await?
                };
                report
                    .cache_stats
                    .add_invalidations(invalidate_applied_actions(
                        &self.reconciliation_detail_cache,
                        self.repo,
                        &outcome,
                    ));
                if !outcome.advisory.is_empty() {
                    self.advisory_actions
                        .fetch_add(saturating_u64(outcome.advisory.len()), Ordering::Relaxed);
                }
                let progress = Progress {
                    changed: !outcome.applied.is_empty(),
                    actions: saturating_u32(outcome.applied.len()),
                };
                log_mechanical_reconciliation_summary(
                    &self.name, self.repo, mode, &report, &outcome, progress,
                );
                Ok::<_, WorkerError>(progress)
            },
        )
        .await?;
        if mode == ReconciliationMode::DeepAudit {
            return Ok(reconciliation_progress);
        }

        let automation_items = measure_mechanical_phase(
            self.forge,
            self.repo,
            "broad",
            None,
            "automated_scan",
            async {
                if !self
                    .compiled
                    .queues()
                    .iter()
                    .any(|queue| queue.automation.is_some())
                {
                    return Ok(Vec::new());
                }
                crate::scan::scan_automated_queues(
                    self.forge,
                    self.repo,
                    self.workflow,
                    &self.compiled,
                    now,
                )
                .await
                .map_err(WorkerError::from)
            },
        )
        .await?;
        let automation_progress = measure_mechanical_phase(
            self.forge,
            self.repo,
            "broad",
            None,
            "transition_application",
            automation::execute_automated_items(
                &self.name,
                self.repo,
                self.workflow,
                &self.compiled,
                &self.executor,
                &self.external_tool_executors,
                self.forge,
                automation_items,
                self.pull_request_merge_observer.as_ref(),
            ),
        )
        .await?;
        if automation_progress.changed {
            self.reconciliation_detail_cache
                .invalidate_repository(self.repo);
        }
        Ok(combine_progress(
            reconciliation_progress,
            automation_progress,
        ))
    }
}

#[async_trait]
impl<F, J, P> Worker for MechanicalWorker<'_, F, J, P>
where
    F: Forge + ?Sized,
    J: temper_workflow::CommandJournal,
    P: RecoveryPolicy + Send + Sync,
{
    async fn tick(&self, now: DateTime<Utc>) -> Result<Progress, WorkerError> {
        self.tick_with_reconciliation_mode(now, ReconciliationMode::Bounded)
            .await
    }

    fn name(&self) -> &str {
        &self.name
    }
}

fn invalidate_applied_actions(
    cache: &ReconciliationDetailCache,
    repo: &RepositoryId,
    outcome: &ApplyOutcome,
) -> usize {
    outcome
        .applied
        .iter()
        .filter_map(recovery_action_target)
        .map(|target| cache.invalidate(repo, target))
        .sum()
}

fn recovery_action_target(action: &RecoveryAction) -> Option<temper_workflow::ArtifactSource> {
    match action {
        RecoveryAction::ConvergeAssignment { target, .. }
        | RecoveryAction::RequeueLease { target }
        | RecoveryAction::Repair { target, .. }
        | RecoveryAction::Unblock { target, .. } => Some(*target),
        RecoveryAction::MarkReconciled { .. }
        | RecoveryAction::Escalate { .. }
        | RecoveryAction::Diagnose { .. } => None,
    }
}

fn combine_progress(left: Progress, right: Progress) -> Progress {
    Progress {
        changed: left.changed || right.changed,
        actions: left.actions.saturating_add(right.actions),
    }
}
