//! Process-split stage sketch for the filesystem topology.

use super::{
    InProcessWorkerContext, InProcessWorkerFactory, ProcessHandleFactory, RoleAuditWorker, Stage,
    StageError, WorkerProcess, provision_labels, role_uses_test_audit_backstop,
};
use crate::config::RunnerConfig;
use crate::driver::{FixpointDriver, ManualClock, RunReport};
use crate::{AgentRegistry, MechanicalWorker, RoleWorker, Worker};
use async_trait::async_trait;
use std::sync::Arc;
use temper_forge_model::{Forge, RepositoryId};
use temper_workflow::{CompiledWorkflow, InMemoryJournal, LeasePolicy, ValidatedWorkflow};

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

    fn role_entries(&self) -> Vec<(temper_workflow::RoleId, Arc<dyn crate::Agent<F>>)> {
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
            if role_uses_test_audit_backstop(role) {
                workers.push(Box::new(RoleAuditWorker::new(
                    role,
                    RoleWorker::new(
                        &self.workflow,
                        &self.compiled,
                        forge,
                        &self.repo,
                        role.clone(),
                        Arc::clone(agent),
                        self.config.execution_context(role),
                    ),
                )));
            }
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
