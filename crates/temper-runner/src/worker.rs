//! Worker primitives for driving runner progress.
//!
//! A [`Worker`] is the unit a driver ticks. [`RoleWorker`] is the per-role
//! production worker: every tick scans fresh Forge state for that role and lets
//! the role's [`Agent`] service each active [`WorkItem`] through [`RoleTools`].
//! [`MechanicalWorker`] is the controller-plane worker: every tick runs the
//! workflow reconciler and recovery applier so expired leases, interrupted
//! commands, dependency unblocks, and stale journal entries converge without
//! spawning an agent.

use crate::agent::{Agent, AgentError, RoleTools};
use crate::observability::{
    render_mechanical_reconciliation_event, render_scan_summary_event,
    render_work_item_selected_event, MechanicalReconciliationEvent, ScanSummaryEvent,
    WorkItemSelectedEvent,
};
use crate::scan::{scan_role, ScanError, WorkItem};
use crate::signal::CiError;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use temper_forge::{Forge, ForgeError, RepositoryId};
use temper_workflow::{
    Applier, ApplyError, CompiledWorkflow, DefaultRecoveryPolicy, ExecutionContext, ExecutionError,
    Executor, LeaseManager, LeasePolicy, ReconcileError, RecoveryPolicy, RoleId, ValidatedWorkflow,
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
        let tools = RoleTools::new(
            self.workflow,
            self.forge,
            self.repo,
            self.role.clone(),
            self.tools.execution_context(),
        )
        .with_observability_tick_id(tick_id.to_string());
        self.tick_with_tools(now, &tools).await
    }

    async fn tick_with_tools(
        &self,
        now: DateTime<Utc>,
        tools: &RoleTools<'_, F>,
    ) -> Result<Progress, WorkerError> {
        let items = scan_role(
            self.forge,
            self.repo,
            self.workflow,
            self.compiled,
            now,
            &self.role,
        )
        .await?;

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

#[async_trait]
impl<F: Forge + ?Sized> Worker for RoleWorker<'_, F> {
    async fn tick(&self, now: DateTime<Utc>) -> Result<Progress, WorkerError> {
        self.tick_with_tools(now, &self.tools).await
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// Controller-plane worker that runs mechanical recovery once per tick.
///
/// The worker owns the reusable runtime components for the process — an
/// [`Executor`] and [`LeaseManager`] bound to `forge` — and borrows the process's
/// [`CommandJournal`](temper_workflow::CommandJournal). `Escalate` and
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
    forge: &'a F,
    repo: &'a RepositoryId,
    executor: Executor<'a, F>,
    lease_manager: LeaseManager<'a, F>,
    journal: &'a J,
    policy: P,
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
            forge,
            repo,
            executor: Executor::new(workflow, forge),
            lease_manager: LeaseManager::new(forge, lease_policy),
            journal,
            policy,
            advisory_actions: AtomicU64::new(0),
        }
    }

    /// Number of advisory recovery actions observed across ticks.
    pub fn advisory_actions(&self) -> u64 {
        self.advisory_actions.load(Ordering::Relaxed)
    }

    /// Command journal this worker reconciles and updates.
    pub fn journal(&self) -> &J {
        self.journal
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
        let reconciler = self.workflow.reconciler(&self.policy);
        let report = reconciler
            .reconcile(self.forge, self.repo, self.journal, now)
            .await?;
        if report.is_clean() {
            return Ok(Progress::unchanged());
        }
        log_mechanical_reconciliation(&self.name, self.repo, &report);

        let outcome = Applier::new(&self.executor, &self.lease_manager, self.journal)
            .apply_report(self.repo, &report, now)
            .await?;
        if !outcome.advisory.is_empty() {
            self.advisory_actions
                .fetch_add(saturating_u64(outcome.advisory.len()), Ordering::Relaxed);
        }

        Ok(Progress {
            changed: !outcome.applied.is_empty(),
            actions: saturating_u32(outcome.applied.len()),
        })
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

fn saturating_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn saturating_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}
