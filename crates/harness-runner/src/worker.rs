//! Worker primitives for driving runner progress.
//!
//! A [`Worker`] is the unit a driver ticks. [`RoleWorker`] is the per-role
//! production worker: every tick scans fresh Forge state for that role and lets
//! the role's [`Agent`] service each active [`WorkItem`] through [`RoleTools`].

use crate::agent::{Agent, AgentError, RoleTools};
use crate::scan::{scan_role, ScanError};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use harness_forge::{Forge, ForgeError, RepositoryId};
use harness_workflow::{
    CompiledWorkflow, ExecutionContext, ExecutionError, RoleId, ValidatedWorkflow,
};
use std::error::Error;
use std::fmt;
use std::sync::Arc;

/// Progress made by one worker tick.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub struct Progress {
    /// Whether the tick changed workflow state.
    pub changed: bool,
    /// Number of serviced work items that reported a workflow-state change.
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
    /// The worker's agent failed while servicing work.
    Agent(AgentError),
}

impl fmt::Display for WorkerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WorkerError::Scan(error) => write!(formatter, "worker scan failed: {error}"),
            WorkerError::Execution(error) => write!(formatter, "worker execution failed: {error}"),
            WorkerError::Forge(error) => {
                write!(formatter, "worker forge operation failed: {error}")
            }
            WorkerError::Agent(error) => write!(formatter, "worker agent failed: {error}"),
        }
    }
}

impl Error for WorkerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            WorkerError::Scan(error) => Some(error),
            WorkerError::Execution(error) => Some(error),
            WorkerError::Forge(error) => Some(error),
            WorkerError::Agent(error) => Some(error),
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

impl From<AgentError> for WorkerError {
    fn from(error: AgentError) -> Self {
        Self::Agent(error)
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
}

#[async_trait]
impl<F: Forge + ?Sized> Worker for RoleWorker<'_, F> {
    async fn tick(&self, now: DateTime<Utc>) -> Result<Progress, WorkerError> {
        let items = scan_role(
            self.forge,
            self.repo,
            self.workflow,
            self.compiled,
            now,
            &self.role,
        )
        .await?;

        let mut progress = Progress::unchanged();
        for item in items {
            progress.record(self.agent.service(&item, &self.tools).await?);
        }
        Ok(progress)
    }

    fn name(&self) -> &str {
        &self.name
    }
}
