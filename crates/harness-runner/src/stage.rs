//! Stage and scenario composition seams.
//!
//! A [`Stage`] owns a runnable world and exposes only a Forge observer plus a
//! `run_to_quiescence` driver hook. A [`Scenario`] seeds outside-world input and
//! asserts the final state exclusively through the portable Forge interface, so
//! the same scenario can run at wider backend/topology boundaries.

use crate::config::{RoleBinding, RunnerConfig};
use crate::driver::{DriveError, FixpointDriver, ManualClock, RunReport};
use crate::{AgentRegistry, MechanicalWorker, RoleWorker, Worker};
use async_trait::async_trait;
use harness_forge::{Forge, ForgeError, RepositoryId, UpsertLabel};
use harness_workflow::{CompiledWorkflow, InMemoryJournal, LeasePolicy, ValidatedWorkflow};
use std::error::Error;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Default tick budget used by [`run_scenario`].
pub const DEFAULT_SCENARIO_BUDGET: u64 = 100;

/// Boxed error type used by scenario seed/assert closures.
pub type BoxError = Box<dyn Error + Send + Sync + 'static>;

/// Future returned by a scenario seed or assertion step.
pub type ScenarioFuture<'a> = Pin<Box<dyn Future<Output = Result<(), BoxError>> + Send + 'a>>;

/// Boxed scenario step operating only through the Forge interface.
pub type ScenarioStep =
    Box<dyn for<'a> Fn(&'a dyn Forge, &'a RepositoryId) -> ScenarioFuture<'a> + Send + Sync>;

/// Backend- and topology-agnostic scenario definition.
pub struct Scenario {
    /// Human-readable scenario name.
    pub name: String,
    /// Outside-world input producer, such as a human filing an issue.
    pub seed: ScenarioStep,
    /// End-state assertion that reads only through Forge.
    pub assert: ScenarioStep,
}

impl Scenario {
    /// Creates a scenario from boxed seed and assertion steps.
    pub fn new(name: impl Into<String>, seed: ScenarioStep, assert: ScenarioStep) -> Self {
        Self {
            name: name.into(),
            seed,
            assert,
        }
    }
}

/// Error from stage construction or execution.
#[derive(Debug)]
pub enum StageError {
    /// Backend operation failed.
    Forge(ForgeError),
    /// The fixpoint driver failed.
    Drive(DriveError),
    /// A registered role worker had no configured identity.
    MissingRoleBinding { role: harness_workflow::RoleId },
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

/// Error returned by [`run_scenario`] helpers.
#[derive(Debug)]
pub enum ScenarioError {
    /// Scenario seeding failed.
    Seed { scenario: String, source: BoxError },
    /// Stage execution failed.
    Run {
        scenario: String,
        source: StageError,
    },
    /// Scenario assertion failed.
    Assert { scenario: String, source: BoxError },
}

impl fmt::Display for ScenarioError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScenarioError::Seed { scenario, source } => {
                write!(formatter, "scenario {scenario} seed failed: {source}")
            }
            ScenarioError::Run { scenario, source } => {
                write!(formatter, "scenario {scenario} run failed: {source}")
            }
            ScenarioError::Assert { scenario, source } => {
                write!(formatter, "scenario {scenario} assertion failed: {source}")
            }
        }
    }
}

impl Error for ScenarioError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ScenarioError::Seed { source, .. } | ScenarioError::Assert { source, .. } => {
                Some(source.as_ref())
            }
            ScenarioError::Run { source, .. } => Some(source),
        }
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
    pub workflow: &'a ValidatedWorkflow,
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

/// Single-process stage over one shared backend store.
pub struct InProcessStage<F: Forge> {
    forge: F,
    repo: RepositoryId,
    workflow: ValidatedWorkflow,
    compiled: CompiledWorkflow,
    config: RunnerConfig,
    agents: AgentRegistry<F>,
    journal: InMemoryJournal,
    identity: IdentityFactory<F>,
    extra_worker_factories: Vec<Arc<dyn InProcessWorkerFactory<F>>>,
    clock: ManualClock,
}

impl<F> InProcessStage<F>
where
    F: Forge + Clone,
{
    /// Creates a stage whose role workers use cloned copies of `forge`.
    pub async fn new(
        forge: F,
        workflow: ValidatedWorkflow,
        config: RunnerConfig,
        agents: AgentRegistry<F>,
    ) -> Result<Self, StageError> {
        Self::with_identity(forge, workflow, config, agents, |forge, _| forge.clone()).await
    }
}

impl<F> InProcessStage<F>
where
    F: Forge,
{
    /// Creates a stage with an explicit per-role identity handle factory.
    ///
    /// Memory tests should pass `|forge, binding| forge.as_user(binding.user.clone())`.
    /// Other backends can clone an already-authenticated handle or provide their
    /// own process identity adapter without changing stage orchestration.
    pub async fn with_identity<I>(
        forge: F,
        workflow: ValidatedWorkflow,
        config: RunnerConfig,
        agents: AgentRegistry<F>,
        identity: I,
    ) -> Result<Self, StageError>
    where
        I: Fn(&F, &RoleBinding) -> F + Send + Sync + 'static,
    {
        let compiled = workflow.compile();
        let repo = forge.create_repository(config.repository.clone()).await?.id;
        provision_labels(&forge, &repo, &compiled).await?;
        Ok(Self {
            forge,
            repo,
            workflow,
            compiled,
            config,
            agents,
            journal: InMemoryJournal::new(),
            identity: Arc::new(identity),
            extra_worker_factories: Vec::new(),
            clock: ManualClock::default(),
        })
    }

    /// Adds an optional worker factory, preserving the stage for chaining.
    pub fn with_extra_worker_factory<T>(mut self, factory: T) -> Self
    where
        T: InProcessWorkerFactory<F> + 'static,
    {
        self.extra_worker_factories.push(Arc::new(factory));
        self
    }

    /// Replaces the stage clock, preserving the stage for chaining.
    pub fn with_clock(mut self, clock: ManualClock) -> Self {
        self.clock = clock;
        self
    }

    /// Returns the stage clock handle.
    pub fn clock(&self) -> &ManualClock {
        &self.clock
    }

    /// Returns the command journal shared by this in-process stage.
    pub fn journal(&self) -> &InMemoryJournal {
        &self.journal
    }

    fn role_entries(&self) -> Vec<(harness_workflow::RoleId, Arc<dyn crate::Agent<F>>)> {
        self.compiled
            .roles()
            .iter()
            .filter_map(|role| {
                self.agents
                    .get(&role.id)
                    .map(|agent| (role.id.clone(), Arc::clone(agent)))
            })
            .collect()
    }
}

/// Stage sketch for the process-split filesystem topology.
///
/// The implementation still runs in one test process, but every worker is
/// built from a handle returned by `handle_factory`. For the L4 rehearsal, pass
/// a factory that creates a fresh `FilesystemForge::new(root)` handle for each
/// [`WorkerProcess`] and applies a `FilesystemForge::as_user`-shaped identity
/// for role workers. Coordination then happens only through
/// the shared Forge store, matching the next phase's one-OS-process-per-worker
/// deployment without introducing binaries yet.
pub struct MultiProcessStage<F: Forge> {
    forge: F,
    repo: RepositoryId,
    workflow: ValidatedWorkflow,
    compiled: CompiledWorkflow,
    config: RunnerConfig,
    agents: AgentRegistry<F>,
    journal: InMemoryJournal,
    handle_factory: ProcessHandleFactory<F>,
    extra_worker_factories: Vec<Arc<dyn InProcessWorkerFactory<F>>>,
    clock: ManualClock,
}

impl<F> MultiProcessStage<F>
where
    F: Forge,
{
    /// Creates a stage with explicit per-worker Forge handle construction.
    pub async fn with_handle_factory<H>(
        forge: F,
        workflow: ValidatedWorkflow,
        config: RunnerConfig,
        agents: AgentRegistry<F>,
        handle_factory: H,
    ) -> Result<Self, StageError>
    where
        H: for<'a> Fn(&F, WorkerProcess<'a>) -> F + Send + Sync + 'static,
    {
        let compiled = workflow.compile();
        let repo = forge.create_repository(config.repository.clone()).await?.id;
        provision_labels(&forge, &repo, &compiled).await?;
        Ok(Self {
            forge,
            repo,
            workflow,
            compiled,
            config,
            agents,
            journal: InMemoryJournal::new(),
            handle_factory: Arc::new(handle_factory),
            extra_worker_factories: Vec::new(),
            clock: ManualClock::default(),
        })
    }

    /// Adds an optional worker factory, preserving the stage for chaining.
    pub fn with_extra_worker_factory<T>(mut self, factory: T) -> Self
    where
        T: InProcessWorkerFactory<F> + 'static,
    {
        self.extra_worker_factories.push(Arc::new(factory));
        self
    }

    /// Replaces the stage clock, preserving the stage for chaining.
    pub fn with_clock(mut self, clock: ManualClock) -> Self {
        self.clock = clock;
        self
    }

    /// Returns the stage clock handle.
    pub fn clock(&self) -> &ManualClock {
        &self.clock
    }

    /// Returns the command journal used by the mechanical worker process sketch.
    pub fn journal(&self) -> &InMemoryJournal {
        &self.journal
    }

    fn role_entries(&self) -> Vec<(harness_workflow::RoleId, Arc<dyn crate::Agent<F>>)> {
        self.compiled
            .roles()
            .iter()
            .filter_map(|role| {
                self.agents
                    .get(&role.id)
                    .map(|agent| (role.id.clone(), Arc::clone(agent)))
            })
            .collect()
    }
}

#[async_trait]
impl<F> Stage for MultiProcessStage<F>
where
    F: Forge + 'static,
{
    async fn run_to_quiescence(&self, budget: u64) -> Result<RunReport, StageError> {
        let role_entries = self.role_entries();
        let mut role_forges = Vec::with_capacity(role_entries.len());
        for (role, _) in &role_entries {
            let binding = self
                .config
                .role_binding(role)
                .ok_or_else(|| StageError::MissingRoleBinding { role: role.clone() })?;
            role_forges.push((self.handle_factory)(
                &self.forge,
                WorkerProcess::Role(binding),
            ));
        }
        let mechanical_forge = (self.handle_factory)(&self.forge, WorkerProcess::Mechanical);
        let extra_forges: Vec<F> = (0..self.extra_worker_factories.len())
            .map(|index| (self.handle_factory)(&self.forge, WorkerProcess::Extra { index }))
            .collect();

        let mechanical = MechanicalWorker::new(
            &self.workflow,
            &mechanical_forge,
            &self.repo,
            &self.journal,
            LeasePolicy::new(self.config.lease_ttl),
        );
        let mut workers: Vec<Box<dyn Worker + '_>> = vec![Box::new(mechanical)];

        for ((role, agent), forge) in role_entries.iter().zip(role_forges.iter()) {
            workers.push(Box::new(RoleWorker::new(
                &self.workflow,
                &self.compiled,
                forge,
                &self.repo,
                role.clone(),
                Arc::clone(agent),
                self.config.execution_context(role),
            )));
        }

        for (factory, forge) in self.extra_worker_factories.iter().zip(extra_forges.iter()) {
            workers.push(factory.build(InProcessWorkerContext {
                forge,
                repo: &self.repo,
                workflow: &self.workflow,
                compiled: &self.compiled,
                config: &self.config,
            }));
        }

        let worker_refs: Vec<&dyn Worker> = workers
            .iter()
            .map(|worker| worker.as_ref() as &dyn Worker)
            .collect();
        let driver = FixpointDriver::with_clock(worker_refs, self.clock.clone());
        driver.run(budget).await.map_err(StageError::from)
    }

    fn forge(&self) -> &dyn Forge {
        &self.forge
    }

    fn repo(&self) -> &RepositoryId {
        &self.repo
    }
}

#[async_trait]
impl<F> Stage for InProcessStage<F>
where
    F: Forge + 'static,
{
    async fn run_to_quiescence(&self, budget: u64) -> Result<RunReport, StageError> {
        let role_entries = self.role_entries();
        let mut role_forges = Vec::with_capacity(role_entries.len());
        for (role, _) in &role_entries {
            let binding = self
                .config
                .role_binding(role)
                .ok_or_else(|| StageError::MissingRoleBinding { role: role.clone() })?;
            role_forges.push((self.identity)(&self.forge, binding));
        }

        let mechanical = MechanicalWorker::new(
            &self.workflow,
            &self.forge,
            &self.repo,
            &self.journal,
            LeasePolicy::new(self.config.lease_ttl),
        );
        let mut workers: Vec<Box<dyn Worker + '_>> = vec![Box::new(mechanical)];

        for ((role, agent), forge) in role_entries.iter().zip(role_forges.iter()) {
            workers.push(Box::new(RoleWorker::new(
                &self.workflow,
                &self.compiled,
                forge,
                &self.repo,
                role.clone(),
                Arc::clone(agent),
                self.config.execution_context(role),
            )));
        }

        for factory in &self.extra_worker_factories {
            workers.push(factory.build(InProcessWorkerContext {
                forge: &self.forge,
                repo: &self.repo,
                workflow: &self.workflow,
                compiled: &self.compiled,
                config: &self.config,
            }));
        }

        let worker_refs: Vec<&dyn Worker> = workers
            .iter()
            .map(|worker| worker.as_ref() as &dyn Worker)
            .collect();
        let driver = FixpointDriver::with_clock(worker_refs, self.clock.clone());
        driver.run(budget).await.map_err(StageError::from)
    }

    fn forge(&self) -> &dyn Forge {
        &self.forge
    }

    fn repo(&self) -> &RepositoryId {
        &self.repo
    }
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

/// Seeds a scenario, runs the stage with the default budget, and asserts state.
pub async fn run_scenario<S: Stage + ?Sized>(
    stage: &S,
    scenario: &Scenario,
) -> Result<RunReport, ScenarioError> {
    run_scenario_with_budget(stage, scenario, DEFAULT_SCENARIO_BUDGET).await
}

/// Seeds a scenario, runs the stage with `budget`, and asserts state.
pub async fn run_scenario_with_budget<S: Stage + ?Sized>(
    stage: &S,
    scenario: &Scenario,
    budget: u64,
) -> Result<RunReport, ScenarioError> {
    (scenario.seed)(stage.forge(), stage.repo())
        .await
        .map_err(|source| ScenarioError::Seed {
            scenario: scenario.name.clone(),
            source,
        })?;
    let report = stage
        .run_to_quiescence(budget)
        .await
        .map_err(|source| ScenarioError::Run {
            scenario: scenario.name.clone(),
            source,
        })?;
    (scenario.assert)(stage.forge(), stage.repo())
        .await
        .map_err(|source| ScenarioError::Assert {
            scenario: scenario.name.clone(),
            source,
        })?;
    Ok(report)
}
