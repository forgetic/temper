//! Controller-plane worker that runs mechanical recovery and automation.

use super::automation;
use super::{Progress, Worker, WorkerError, saturating_u32, saturating_u64};
use crate::coding_workspace::ExternalToolExecutors;
use crate::observability::{
    MechanicalReconciliationEvent, StructuredEvent, render_mechanical_reconciliation_event,
};
use crate::scan::targeted_automated_work_items;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::sync::atomic::{AtomicU64, Ordering};
use temper_forge_model::{ChangeKind, Forge, ForgeError, ItemNumber, RepositoryId};
use temper_workflow::{
    Applier, ApplyOutcome, ArtifactSnapshot, CompiledWorkflow, DefaultRecoveryPolicy, Executor,
    LeaseManager, LeasePolicy, ReconciliationMode, RecoveryPolicy, ValidatedWorkflow,
    parse_metadata_block,
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
    journal: &'a J,
    policy: P,
    /// Workspace executors the actor roles of workspace-backed automations can
    /// invoke. Empty when no executor is bound; such automations no-op until a
    /// binding exists, never failing the tick.
    external_tool_executors: ExternalToolExecutors,
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
            journal,
            policy,
            external_tool_executors: ExternalToolExecutors::new(),
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

    /// Number of advisory recovery actions observed across ticks.
    pub fn advisory_actions(&self) -> u64 {
        self.advisory_actions.load(Ordering::Relaxed)
    }

    /// Command journal this worker reconciles and updates.
    pub fn journal(&self) -> &J {
        self.journal
    }

    /// Repository path configured for this single-repository worker.
    pub async fn repository_path(&self) -> Result<temper_forge_model::RepositoryPath, WorkerError> {
        let repository = self
            .forge
            .get_repository(self.repo)
            .await?
            .ok_or_else(|| ForgeError::NotFound(format!("repository {}", self.repo)))?;
        Ok(temper_forge_model::RepositoryPath::new(
            repository.owner,
            repository.name,
        ))
    }

    /// Evaluates one changed artifact using exact fetch-by-number APIs.
    pub async fn tick_artifact(
        &self,
        now: DateTime<Utc>,
        item: ItemNumber,
        kind: ChangeKind,
    ) -> Result<Progress, WorkerError> {
        let classifier = temper_workflow::Classifier::new(self.workflow);
        let mut targeted_snapshots = Vec::new();
        let classified = match kind {
            ChangeKind::Ci | ChangeKind::PullRequest => {
                let pull_request = self
                    .forge
                    .get_pull_request_by_number(self.repo, item)
                    .await?;
                if let Some(pull_request) = pull_request {
                    targeted_snapshots.push(ArtifactSnapshot::from_pull_request(&pull_request));
                    if matches!(kind, ChangeKind::PullRequest)
                        && let Some(metadata) = parse_metadata_block(&pull_request.body)
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
                    classifier.classify_pull_request(&pull_request).ok()
                } else {
                    None
                }
            }
            // Issue-like events are routed to issues deterministically. Review
            // hints may come from PR review webhooks, but providers also emit a
            // PullRequest/Ci hint for PR state; keeping Review issue-routed
            // avoids guessing across artifact namespaces.
            ChangeKind::Issue | ChangeKind::Label | ChangeKind::Review | ChangeKind::Comment => {
                self.forge
                    .get_issue_by_number(self.repo, item)
                    .await?
                    .and_then(|issue| classifier.classify_issue(&issue).ok())
            }
            ChangeKind::Push | ChangeKind::Unknown => return Ok(Progress::unchanged()),
        };
        let Some(classified) = classified else {
            return Ok(Progress::unchanged());
        };
        let automation_items = targeted_automated_work_items(
            self.forge,
            self.repo,
            self.workflow,
            &self.compiled,
            classified,
            now,
        )
        .await?;
        let automation_progress = automation::execute_automated_items(
            &self.name,
            self.repo,
            self.workflow,
            &self.compiled,
            &self.executor,
            &self.external_tool_executors,
            self.forge,
            automation_items,
        )
        .await?;
        if targeted_snapshots.is_empty() {
            return Ok(automation_progress);
        }
        let reconciliation_progress = self
            .reconcile_targeted_snapshots(now, targeted_snapshots)
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
                .apply_report(self.repo, &report, now)
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
        let reconciler = self.workflow.reconciler(&self.policy);
        let report = reconciler
            .reconcile_with_mode(self.forge, self.repo, self.journal, mode, now)
            .await?;
        let outcome = if report.is_clean() {
            ApplyOutcome::default()
        } else {
            log_mechanical_reconciliation(&self.name, self.repo, &report);
            Applier::new(&self.executor, &self.lease_manager, self.journal)
                .apply_report(self.repo, &report, now)
                .await?
        };
        if !outcome.advisory.is_empty() {
            self.advisory_actions
                .fetch_add(saturating_u64(outcome.advisory.len()), Ordering::Relaxed);
        }
        let reconciliation_progress = Progress {
            changed: !outcome.applied.is_empty(),
            actions: saturating_u32(outcome.applied.len()),
        };
        log_mechanical_reconciliation_summary(
            &self.name,
            self.repo,
            mode,
            &report,
            &outcome,
            reconciliation_progress,
        );
        if mode == ReconciliationMode::DeepAudit {
            return Ok(reconciliation_progress);
        }

        let automation_progress = automation::execute_automated_queues(
            &self.name,
            self.repo,
            self.workflow,
            &self.compiled,
            &self.executor,
            &self.external_tool_executors,
            self.forge,
            now,
        )
        .await?;
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

fn log_mechanical_reconciliation(
    worker: &str,
    repo: &RepositoryId,
    report: &temper_workflow::ReconcileReport,
) {
    for (finding, action) in report.findings.iter().zip(report.actions.iter()) {
        eprintln!(
            "{}",
            render_mechanical_reconciliation_event(&MechanicalReconciliationEvent {
                worker,
                repo,
                finding,
                action,
            })
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
    eprintln!(
        "{}",
        StructuredEvent::new("mechanical_reconciliation_summary")
            .string("worker_kind", "mechanical")
            .string("worker", worker)
            .string("repo", repo.to_string())
            .string("mode", reconciliation_mode_name(mode))
            .number("snapshot_count", saturating_u64(report.snapshot_count))
            .number("finding_count", saturating_u64(report.findings.len()))
            .number(
                "recovery_action_count",
                saturating_u64(report.actions.len())
            )
            .number(
                "applied_action_count",
                saturating_u64(outcome.applied.len())
            )
            .number(
                "advisory_action_count",
                saturating_u64(outcome.advisory.len())
            )
            .boolean("changed", progress.changed)
            .number("progress_actions", u64::from(progress.actions))
            .render()
    );
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
