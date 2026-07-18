//! Controller-plane worker that runs mechanical recovery and automation.

use super::automation;
use super::{Progress, Worker, WorkerError, saturating_u32, saturating_u64};
use crate::coding_workspace::ExternalToolExecutors;
use crate::observability::artifact_ref;
use crate::scan::{ArtifactAddress, TargetedArtifactSnapshot, load_targeted_artifact};
use crate::worker::PullRequestMergeObserver;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use temper_forge::{ChangeKind, Forge, ForgeError, HintArtifactKind, ItemNumber, RepositoryId};
use temper_log::strip_provider_scheme;
use temper_workflow::{
    Applier, ApplyOutcome, ArtifactSnapshot, AssignmentConverger, CompiledWorkflow,
    DefaultRecoveryPolicy, Executor, LeaseManager, LeasePolicy, ReconciliationMode, RecoveryPolicy,
    ValidatedWorkflow, parse_metadata_block,
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
            external_tool_executors: ExternalToolExecutors::new(),
            pull_request_merge_observer: None,
            advisory_actions: AtomicU64::new(0),
        }
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
                let mut targeted_snapshots = Vec::new();
                let Some(loaded) =
                    load_targeted_artifact(self.forge, self.repo, self.workflow, address).await?
                else {
                    return Ok(None);
                };
                // Staged fan-out children are not externally dispatchable yet.
                // This guard must precede both targeted automation and
                // reconciliation so a webhook cannot mutate a partially wired
                // child.
                if loaded.classified.metadata.staged {
                    return Ok(None);
                }
                if let TargetedArtifactSnapshot::PullRequest(pull_request) = &loaded.snapshot {
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
                                    targeted_snapshots.push(ArtifactSnapshot::from_issue(&issue));
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
        let report = reconciler
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
                let report = reconciler
                    .reconcile_with_mode(self.forge, self.repo, self.journal, mode, now)
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
        Ok(combine_progress(
            reconciliation_progress,
            automation_progress,
        ))
    }
}

/// Runs one expensive mechanical phase and emits exactly one terminal debug
/// measurement. The optional backend counter is sampled around the phase so
/// Forgejo-backed runs expose a request delta without making observability part
/// of correctness.
async fn measure_mechanical_phase<F, Fut, T, E>(
    forge: &F,
    repo: &RepositoryId,
    scope: &'static str,
    address: Option<ArtifactAddress>,
    phase: &'static str,
    future: Fut,
) -> Result<T, E>
where
    F: Forge + ?Sized,
    Fut: Future<Output = Result<T, E>>,
{
    let started = Instant::now();
    let provider_requests_before = forge.provider_request_count();
    let result = future.await;
    let provider_requests = provider_requests_before.and_then(|before| {
        forge
            .provider_request_count()
            .map(|after| after.saturating_sub(before))
    });
    let outcome = if result.is_ok() { "success" } else { "failed" };
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let provider_request_total = provider_requests.unwrap_or(0);
    let provider_requests_available = provider_requests.is_some();
    let repository = strip_provider_scheme(repo.as_str());
    if let Some(address) = address {
        let artifact = artifact_ref(repo, address.source()).to_string();
        tracing::debug!(
            target: "temper::worker",
            measurement = "mechanical.phase",
            repo = repository,
            mechanical.scope = scope,
            mechanical.phase = phase,
            artifact.ref = artifact,
            outcome,
            duration_ms,
            provider.request_total = provider_request_total,
            provider.requests_available = provider_requests_available,
            "mechanical {scope} {phase} {outcome}"
        );
    } else {
        tracing::debug!(
            target: "temper::worker",
            measurement = "mechanical.phase",
            repo = repository,
            mechanical.scope = scope,
            mechanical.phase = phase,
            outcome,
            duration_ms,
            provider.request_total = provider_request_total,
            provider.requests_available = provider_requests_available,
            "mechanical {scope} {phase} {outcome}"
        );
    }
    result
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

/// Logs mechanical recovery findings/actions at debug.
///
/// Reconciliation recovery is a §5 "between" cause (lease requeues, drift
/// repairs, advisory diagnoses), not a §7 workflow state change, so it stays at
/// debug under the worker target. The names are the same compact, body-free
/// tokens the old structured event used.
fn log_mechanical_reconciliation(
    worker: &str,
    repo: &RepositoryId,
    report: &temper_workflow::ReconcileReport,
) {
    for (finding, action) in report.findings.iter().zip(report.actions.iter()) {
        tracing::debug!(
            target: "temper::worker",
            worker_kind = "mechanical",
            worker,
            repo = repo.as_str(),
            finding = finding_name(finding),
            action = action_name(action),
            "reconcile: {} -> {}",
            finding_name(finding),
            action_name(action),
        );
    }
}

fn log_mechanical_reconciliation_summary(
    worker: &str,
    repo: &RepositoryId,
    mode: ReconciliationMode,
    report: &temper_workflow::ReconcileReport,
    outcome: &ApplyOutcome,
    progress: Progress,
) {
    tracing::debug!(
        target: "temper::worker",
        worker_kind = "mechanical",
        worker,
        repo = repo.as_str(),
        mode = reconciliation_mode_name(mode),
        snapshot_count = saturating_u64(report.snapshot_count),
        finding_count = saturating_u64(report.findings.len()),
        recovery_action_count = saturating_u64(report.actions.len()),
        applied_action_count = saturating_u64(outcome.applied.len()),
        advisory_action_count = saturating_u64(outcome.advisory.len()),
        changed = progress.changed,
        progress_actions = u64::from(progress.actions),
        "reconcile {} pass: {} finding(s), {} applied",
        reconciliation_mode_name(mode),
        report.findings.len(),
        outcome.applied.len(),
    );
}

fn finding_name(finding: &temper_workflow::ReconcileFinding) -> &'static str {
    use temper_workflow::ReconcileFinding;
    match finding {
        ReconcileFinding::ExpiredAssignment { .. } => "expired_assignment",
        ReconcileFinding::ExpiredLease { .. } => "expired_lease",
        ReconcileFinding::ImpossibleState { .. } => "impossible_state",
        ReconcileFinding::ClassificationDrift { .. } => "classification_drift",
        ReconcileFinding::BlockedWithoutDependencies { .. } => "blocked_without_dependencies",
        ReconcileFinding::PartialTransition { .. } => "partial_transition",
        ReconcileFinding::StaleCommand { .. } => "stale_command",
        ReconcileFinding::DependenciesResolved { .. } => "dependencies_resolved",
    }
}

fn action_name(action: &temper_workflow::RecoveryAction) -> &'static str {
    use temper_workflow::RecoveryAction;
    match action {
        RecoveryAction::ConvergeAssignment { .. } => "converge_assignment",
        RecoveryAction::RequeueLease { .. } => "requeue_lease",
        RecoveryAction::Escalate { .. } => "escalate",
        RecoveryAction::Repair { .. } => "repair",
        RecoveryAction::MarkReconciled { .. } => "mark_reconciled",
        RecoveryAction::Unblock { .. } => "unblock",
        RecoveryAction::Diagnose { .. } => "diagnose",
    }
}

fn reconciliation_mode_name(mode: ReconciliationMode) -> &'static str {
    match mode {
        ReconciliationMode::Bounded => "bounded",
        ReconciliationMode::DeepAudit => "deep-audit",
    }
}

fn combine_progress(left: Progress, right: Progress) -> Progress {
    Progress {
        changed: left.changed || right.changed,
        actions: left.actions.saturating_add(right.actions),
    }
}
