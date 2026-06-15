//! Stage and scenario composition seams.
//!
//! A [`Stage`] owns a runnable world and exposes only a Forge observer plus a
//! `run_to_quiescence` driver hook. A [`Scenario`] seeds outside-world input and
//! asserts the final state exclusively through the portable Forge interface, so
//! the same scenario can run at wider backend/topology boundaries.

mod in_process;
mod multi_process;
mod scenario;

use crate::config::RoleBinding;
use crate::config::RunnerConfig;
use crate::driver::{DriveError, RunReport};
use crate::{Progress, RoleWorker, Worker, WorkerError};
use async_trait::async_trait;
use std::error::Error;
use std::fmt;
use std::sync::Arc;
use temper_forge_model::{Forge, ForgeError, RepositoryId, UpsertLabel};
use temper_workflow::{CompiledWorkflow, RoleId};

pub use in_process::InProcessStage;
pub use multi_process::MultiProcessStage;
pub use scenario::{
    BoxError, DEFAULT_SCENARIO_BUDGET, Scenario, ScenarioError, ScenarioFuture, ScenarioStep,
    run_scenario, run_scenario_with_budget,
};

/// Error from stage construction or execution.
#[derive(Debug)]
pub enum StageError {
    /// Backend operation failed.
    Forge(ForgeError),
    /// The fixpoint driver failed.
    Drive(DriveError),
    /// A registered role worker had no configured identity.
    MissingRoleBinding { role: temper_workflow::RoleId },
}

impl fmt::Display for StageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StageError::Forge(error) => write!(formatter, "stage forge setup failed: {error}"),
            StageError::Drive(error) => write!(formatter, "stage driver failed: {error}"),
            StageError::MissingRoleBinding { role } => {
                write!(formatter, "no runner role binding configured for {role}")
            }
        }
    }
}

impl Error for StageError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            StageError::Forge(error) => Some(error),
            StageError::Drive(error) => Some(error),
            StageError::MissingRoleBinding { .. } => None,
        }
    }
}

impl From<ForgeError> for StageError {
    fn from(error: ForgeError) -> Self {
        Self::Forge(error)
    }
}

impl From<DriveError> for StageError {
    fn from(error: DriveError) -> Self {
        Self::Drive(error)
    }
}

/// Runnable world abstraction used by backend/topology-neutral scenarios.
#[async_trait]
pub trait Stage: Send + Sync {
    /// Runs this stage until a fixpoint or `budget` worker ticks.
    async fn run_to_quiescence(&self, budget: u64) -> Result<RunReport, StageError>;

    /// Forge observer used by scenarios for seeding and assertions.
    fn forge(&self) -> &dyn Forge;

    /// Repository under test.
    fn repo(&self) -> &RepositoryId;
}

/// Context passed to optional in-process worker factories.
pub struct InProcessWorkerContext<'a, F: Forge> {
    /// Base stage Forge handle.
    pub forge: &'a F,
    /// Stage repository.
    pub repo: &'a RepositoryId,
    /// Validated workflow.
    pub workflow: &'a temper_workflow::ValidatedWorkflow,
    /// Compiled workflow manifests.
    pub compiled: &'a CompiledWorkflow,
    /// Topology-independent runner configuration.
    pub config: &'a RunnerConfig,
}

/// Factory for optional workers such as the phase-04b CI producer.
pub trait InProcessWorkerFactory<F: Forge>: Send + Sync {
    /// Builds a worker borrowing from the current stage run.
    fn build<'a>(&self, context: InProcessWorkerContext<'a, F>) -> Box<dyn Worker + 'a>;
}

impl<F, T> InProcessWorkerFactory<F> for T
where
    F: Forge,
    T: Send + Sync + for<'a> Fn(InProcessWorkerContext<'a, F>) -> Box<dyn Worker + 'a>,
{
    fn build<'a>(&self, context: InProcessWorkerContext<'a, F>) -> Box<dyn Worker + 'a> {
        (self)(context)
    }
}

type IdentityFactory<F> = Arc<dyn Fn(&F, &RoleBinding) -> F + Send + Sync>;
type ProcessHandleFactory<F> = Arc<dyn for<'a> Fn(&F, WorkerProcess<'a>) -> F + Send + Sync>;

/// Worker slot requesting its own Forge handle in a process-split stage.
#[derive(Clone, Copy, Debug)]
pub enum WorkerProcess<'a> {
    /// Controller-plane reconcile/apply worker.
    Mechanical,
    /// Role worker bound to a configured identity.
    Role(&'a RoleBinding),
    /// Optional worker factory, such as the test-only CI producer.
    Extra { index: usize },
}

struct RoleAuditWorker<'a, F: Forge + ?Sized> {
    name: String,
    inner: RoleWorker<'a, F>,
}

impl<'a, F: Forge + ?Sized> RoleAuditWorker<'a, F> {
    fn new(role: &RoleId, inner: RoleWorker<'a, F>) -> Self {
        Self {
            name: format!("role-audit:{role}"),
            inner,
        }
    }
}

#[async_trait]
impl<F: Forge + ?Sized> Worker for RoleAuditWorker<'_, F> {
    async fn tick(&self, now: chrono::DateTime<chrono::Utc>) -> Result<Progress, WorkerError> {
        self.inner.tick_audit(now).await
    }

    fn name(&self) -> &str {
        &self.name
    }
}

fn role_uses_test_audit_backstop(role: &RoleId) -> bool {
    // The deterministic in-process scenario stages have no wall-clock audit
    // cadence. Run the reference architect's audit path as an explicit test
    // backstop so terminal recovery queues such as `landed_inbox` remain covered
    // even though normal role scans intentionally skip closed/merged artifacts.
    role.as_str() == "architect"
}

async fn provision_labels<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    compiled: &CompiledWorkflow,
) -> Result<(), ForgeError> {
    for label in compiled.labels().labels() {
        forge
            .upsert_label(
                repo,
                UpsertLabel {
                    name: label.id.to_string(),
                    color: None,
                    description: None,
                },
            )
            .await?;
    }
    Ok(())
}
