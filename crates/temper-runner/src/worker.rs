//! Worker primitives for driving runner progress.
//!
//! A [`Worker`] is the unit a driver ticks. [`RoleWorker`] is the per-role
//! production worker: every tick scans fresh Forge state for that role and lets
//! the role's [`Agent`] service each active [`WorkItem`] through [`RoleTools`].
//! [`MechanicalWorker`] is the controller-plane worker: every normal tick runs
//! bounded reconciliation/recovery and then services declared automated queues
//! through workflow transitions, so mechanical state changes converge without
//! spawning an agent.

mod automation;

use crate::agent::{Agent, AgentError, RoleTools};
use crate::coding_workspace::ExternalToolExecutors;
use crate::observability::{
    MechanicalReconciliationEvent, ScanSummaryEvent, StructuredEvent, WorkItemSelectedEvent,
    render_mechanical_reconciliation_event, render_scan_summary_event,
    render_work_item_selected_event,
};
use crate::scan::{
    ScanError, WorkItem, scan_role, scan_role_audit, scan_role_wake, targeted_automated_work_items,
};
use crate::signal::CiError;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use temper_forge::{ChangeKind, Forge, ForgeError, ItemNumber, RepositoryId};
use temper_workflow::{
    Applier, ApplyError, ApplyOutcome, ArtifactSnapshot, CompiledWorkflow, DefaultRecoveryPolicy,
    ExecutionContext, ExecutionError, Executor, LeaseManager, LeasePolicy, ReconcileError,
    ReconciliationMode, RecoveryPolicy, RoleId, ValidatedWorkflow, parse_metadata_block,
};

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

/// Errors that can stop a worker tick.
#[derive(Debug)]
pub enum WorkerError {
    /// Queue scanning failed.
    Scan(ScanError),
    /// Workflow execution failed outside an agent boundary.
    Execution(ExecutionError),
    /// A direct Forge operation failed outside an agent boundary.
    Forge(ForgeError),
    /// Reconciliation could not load Forge or journal state.
    Reconcile(ReconcileError),
    /// Applying a reconciliation report failed.
    Apply(ApplyError),
    /// The worker's agent failed while servicing work.
    Agent(AgentError),
    /// A fake outside-world CI producer failed.
    Ci(CiError),
    /// One or more repositories failed in a multi-repo wrapper tick.
    MultiRepo(crate::multi_repo::MultiRepoError),
}

impl fmt::Display for WorkerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WorkerError::Scan(error) => write!(formatter, "worker scan failed: {error}"),
            WorkerError::Execution(error) => write!(formatter, "worker execution failed: {error}"),
            WorkerError::Forge(error) => {
                write!(formatter, "worker forge operation failed: {error}")
            }
            WorkerError::Reconcile(error) => write!(formatter, "worker reconcile failed: {error}"),
            WorkerError::Apply(error) => write!(formatter, "worker recovery apply failed: {error}"),
            WorkerError::Agent(error) => write!(formatter, "worker agent failed: {error}"),
            WorkerError::Ci(error) => write!(formatter, "worker CI producer failed: {error}"),
            WorkerError::MultiRepo(error) => write!(formatter, "multi-repo worker failed: {error}"),
        }
    }
}

impl Error for WorkerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            WorkerError::Scan(error) => Some(error),
            WorkerError::Execution(error) => Some(error),
            WorkerError::Forge(error) => Some(error),
            WorkerError::Reconcile(error) => Some(error),
            WorkerError::Apply(error) => Some(error),
            WorkerError::Agent(error) => Some(error),
            WorkerError::Ci(error) => Some(error),
            WorkerError::MultiRepo(error) => Some(error),
        }
    }
}

impl From<ScanError> for WorkerError {
    fn from(error: ScanError) -> Self {
        Self::Scan(error)
    }
}

impl From<ExecutionError> for WorkerError {
    fn from(error: ExecutionError) -> Self {
        Self::Execution(error)
    }
}

impl From<ForgeError> for WorkerError {
    fn from(error: ForgeError) -> Self {
        Self::Forge(error)
    }
}

impl From<ReconcileError> for WorkerError {
    fn from(error: ReconcileError) -> Self {
        Self::Reconcile(error)
    }
}

impl From<ApplyError> for WorkerError {
    fn from(error: ApplyError) -> Self {
        Self::Apply(error)
    }
}

impl From<AgentError> for WorkerError {
    fn from(error: AgentError) -> Self {
        Self::Agent(error)
    }
}

impl From<CiError> for WorkerError {
    fn from(error: CiError) -> Self {
        Self::Ci(error)
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

/// Per-role worker that scans active queues and delegates behavior to an agent.
pub struct RoleWorker<'a, F: Forge + ?Sized> {
    name: String,
    forge: &'a F,
    repo: &'a RepositoryId,
    workflow: &'a ValidatedWorkflow,
    compiled: &'a CompiledWorkflow,
    role: RoleId,
    agent: Arc<dyn Agent<F> + 'a>,
    tools: RoleTools<'a, F>,
}

impl<'a, F: Forge + ?Sized> RoleWorker<'a, F> {
    /// Creates a role worker with the default `role:<id>` name.
    pub fn new(
        workflow: &'a ValidatedWorkflow,
        compiled: &'a CompiledWorkflow,
        forge: &'a F,
        repo: &'a RepositoryId,
        role: RoleId,
        agent: Arc<dyn Agent<F> + 'a>,
        context: ExecutionContext,
    ) -> Self {
        let name = format!("role:{role}");
        let tools = RoleTools::new(workflow, forge, repo, role.clone(), context);
        Self {
            name,
            forge,
            repo,
            workflow,
            compiled,
            role,
            agent,
            tools,
        }
    }

    /// Workflow role serviced by this worker.
    pub fn role(&self) -> &RoleId {
        &self.role
    }

    /// Ticks this worker while attaching a production tick id to work-item logs.
    pub async fn tick_with_observability_tick_id(
        &self,
        now: DateTime<Utc>,
        tick_id: &str,
    ) -> Result<Progress, WorkerError> {
        let tools = self.tools_with_observability_tick_id(tick_id);
        self.tick_with_tools(now, &tools, RoleScanMode::Normal)
            .await
    }

    /// Runs an audit scan for this role.
    pub async fn tick_audit(&self, now: DateTime<Utc>) -> Result<Progress, WorkerError> {
        self.tick_with_tools(now, &self.tools, RoleScanMode::Audit)
            .await
    }

    /// Runs a wake scan for this role.
    pub async fn tick_wake(&self, now: DateTime<Utc>) -> Result<Progress, WorkerError> {
        self.tick_with_tools(now, &self.tools, RoleScanMode::Wake)
            .await
    }

    /// Runs a wake scan while attaching a production tick id to work-item logs.
    pub async fn tick_wake_with_observability_tick_id(
        &self,
        now: DateTime<Utc>,
        tick_id: &str,
    ) -> Result<Progress, WorkerError> {
        let tools = self.tools_with_observability_tick_id(tick_id);
        self.tick_with_tools(now, &tools, RoleScanMode::Wake).await
    }

    /// Runs an audit scan while attaching a production tick id to work-item logs.
    pub async fn tick_audit_with_observability_tick_id(
        &self,
        now: DateTime<Utc>,
        tick_id: &str,
    ) -> Result<Progress, WorkerError> {
        let tools = self.tools_with_observability_tick_id(tick_id);
        self.tick_with_tools(now, &tools, RoleScanMode::Audit).await
    }

    fn tools_with_observability_tick_id(&self, tick_id: &str) -> RoleTools<'_, F> {
        RoleTools::new(
            self.workflow,
            self.forge,
            self.repo,
            self.role.clone(),
            self.tools.execution_context(),
        )
        .with_observability_tick_id(tick_id.to_string())
    }

    async fn tick_with_tools(
        &self,
        now: DateTime<Utc>,
        tools: &RoleTools<'_, F>,
        mode: RoleScanMode,
    ) -> Result<Progress, WorkerError> {
        let items = match mode {
            RoleScanMode::Normal => {
                scan_role(
                    self.forge,
                    self.repo,
                    self.workflow,
                    self.compiled,
                    now,
                    &self.role,
                )
                .await?
            }
            RoleScanMode::Wake => {
                scan_role_wake(
                    self.forge,
                    self.repo,
                    self.workflow,
                    self.compiled,
                    now,
                    &self.role,
                )
                .await?
            }
            RoleScanMode::Audit => {
                scan_role_audit(
                    self.forge,
                    self.repo,
                    self.workflow,
                    self.compiled,
                    now,
                    &self.role,
                )
                .await?
            }
        };

        log_role_scan(
            &self.name,
            self.repo,
            self.workflow.name(),
            &self.role,
            tools,
            &items,
        );

        let mut progress = Progress::unchanged();
        for item in items {
            progress.record(self.agent.service(&item, tools).await?);
        }
        Ok(progress)
    }
}

#[derive(Clone, Copy)]
enum RoleScanMode {
    Normal,
    Wake,
    Audit,
}

#[async_trait]
impl<F: Forge + ?Sized> Worker for RoleWorker<'_, F> {
    async fn tick(&self, now: DateTime<Utc>) -> Result<Progress, WorkerError> {
        self.tick_with_tools(now, &self.tools, RoleScanMode::Normal)
            .await
    }

    fn name(&self) -> &str {
        &self.name
    }
}

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

fn log_role_scan<F: Forge + ?Sized>(
    worker: &str,
    repo: &RepositoryId,
    workflow_id: &str,
    role: &RoleId,
    tools: &RoleTools<'_, F>,
    items: &[WorkItem],
) {
    let Some(tick_id) = tools.observability_tick_id() else {
        return;
    };
    if items.is_empty() {
        return;
    }
    eprintln!(
        "{}",
        render_scan_summary_event(&ScanSummaryEvent {
            tick_id: Some(tick_id),
            worker_kind: "role",
            worker,
            repo,
            workflow_id,
            role: Some(role.as_str()),
            work_item_count: items.len(),
        })
    );
    for item in items {
        let identity = tools.work_item_identity(item);
        eprintln!(
            "{}",
            render_work_item_selected_event(&WorkItemSelectedEvent {
                identity: &identity,
                workflow_id,
                worker,
            })
        );
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

fn saturating_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn saturating_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}
