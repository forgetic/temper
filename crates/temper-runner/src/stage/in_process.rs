//! Single-process stage over one shared backend store.

use super::{
    IdentityFactory, InProcessWorkerContext, InProcessWorkerFactory, RoleAuditWorker, Stage,
    StageError, provision_labels, role_uses_test_audit_backstop,
};
use crate::config::{RoleBinding, RunnerConfig};
use crate::driver::{FixpointDriver, ManualClock, RunReport};
use crate::{AgentRegistry, MechanicalWorker, RoleWorker, Worker};
use async_trait::async_trait;
use std::sync::Arc;
use temper_forge::{Forge, RepositoryId};
use temper_workflow::{CompiledWorkflow, InMemoryJournal, LeasePolicy, ValidatedWorkflow};

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
