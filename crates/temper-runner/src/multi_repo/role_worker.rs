//! Multi-repository wrapper over the per-repository role worker.

use super::report::MultiRepoTickReport;
use super::repository_set::{RepositorySet, RepositoryTarget};
use crate::{Agent, Progress, RoleWorker, Worker, WorkerError};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use temper_forge_model::{ChangeHint, Forge};
use temper_workflow::{CompiledWorkflow, ExecutionContext, RoleId, ValidatedWorkflow};

pub struct MultiRepoRoleWorker<'a, F: Forge + ?Sized> {
    name: String,
    forge: &'a F,
    repositories: RepositorySet,
    workflow: &'a ValidatedWorkflow,
    compiled: &'a CompiledWorkflow,
    role: RoleId,
    agent: Arc<dyn Agent<F> + 'a>,
    context: ExecutionContext,
}

impl<'a, F: Forge + ?Sized> MultiRepoRoleWorker<'a, F> {
    pub fn new(
        workflow: &'a ValidatedWorkflow,
        compiled: &'a CompiledWorkflow,
        forge: &'a F,
        repositories: RepositorySet,
        role: RoleId,
        agent: Arc<dyn Agent<F> + 'a>,
        context: ExecutionContext,
    ) -> Self {
        Self {
            name: format!("multi-role:{role}"),
            forge,
            repositories,
            workflow,
            compiled,
            role,
            agent,
            context,
        }
    }

    pub fn repositories(&self) -> &RepositorySet {
        &self.repositories
    }

    pub async fn tick_report(&self, now: DateTime<Utc>) -> MultiRepoTickReport {
        self.tick_repositories(
            now,
            self.repositories.iter_refs(),
            None,
            RoleTickMode::Normal,
        )
        .await
    }

    pub async fn tick_hinted(
        &self,
        now: DateTime<Utc>,
        hints: &[ChangeHint],
    ) -> MultiRepoTickReport {
        self.tick_repositories(
            now,
            self.repositories.hinted_order(hints),
            None,
            RoleTickMode::Normal,
        )
        .await
    }

    /// Ticks only repositories matching known repo hints; empty means broad fallback is needed.
    pub async fn tick_matching_hints(
        &self,
        now: DateTime<Utc>,
        hints: &[ChangeHint],
    ) -> MultiRepoTickReport {
        self.tick_repositories(
            now,
            self.repositories.matching_hints(hints),
            None,
            RoleTickMode::Normal,
        )
        .await
    }

    pub async fn tick_hinted_with_observability_tick_id(
        &self,
        now: DateTime<Utc>,
        hints: &[ChangeHint],
        tick_id: &str,
    ) -> MultiRepoTickReport {
        self.tick_repositories(
            now,
            self.repositories.hinted_order(hints),
            Some(tick_id),
            RoleTickMode::Normal,
        )
        .await
    }

    pub async fn tick_matching_hints_with_observability_tick_id(
        &self,
        now: DateTime<Utc>,
        hints: &[ChangeHint],
        tick_id: &str,
    ) -> MultiRepoTickReport {
        self.tick_repositories(
            now,
            self.repositories.matching_hints(hints),
            Some(tick_id),
            RoleTickMode::Normal,
        )
        .await
    }

    pub async fn tick_wake_report(&self, now: DateTime<Utc>) -> MultiRepoTickReport {
        self.tick_repositories(now, self.repositories.iter_refs(), None, RoleTickMode::Wake)
            .await
    }

    pub async fn tick_hinted_wake(
        &self,
        now: DateTime<Utc>,
        hints: &[ChangeHint],
    ) -> MultiRepoTickReport {
        self.tick_repositories(
            now,
            self.repositories.hinted_order(hints),
            None,
            RoleTickMode::Wake,
        )
        .await
    }

    pub async fn tick_matching_hints_wake(
        &self,
        now: DateTime<Utc>,
        hints: &[ChangeHint],
    ) -> MultiRepoTickReport {
        self.tick_repositories(
            now,
            self.repositories.matching_hints(hints),
            None,
            RoleTickMode::Wake,
        )
        .await
    }

    pub async fn tick_audit_report(&self, now: DateTime<Utc>) -> MultiRepoTickReport {
        self.tick_repositories(
            now,
            self.repositories.iter_refs(),
            None,
            RoleTickMode::Audit,
        )
        .await
    }

    pub async fn tick_audit_with_observability_tick_id(
        &self,
        now: DateTime<Utc>,
        tick_id: &str,
    ) -> MultiRepoTickReport {
        self.tick_repositories(
            now,
            self.repositories.iter_refs(),
            Some(tick_id),
            RoleTickMode::Audit,
        )
        .await
    }

    async fn tick_repositories(
        &self,
        now: DateTime<Utc>,
        repositories: Vec<&RepositoryTarget>,
        tick_id: Option<&str>,
        mode: RoleTickMode,
    ) -> MultiRepoTickReport {
        let mut report = MultiRepoTickReport::default();
        for repository in repositories {
            report.record_attempt(repository);
            let worker = RoleWorker::new(
                self.workflow,
                self.compiled,
                self.forge,
                &repository.id,
                self.role.clone(),
                Arc::clone(&self.agent),
                self.context.clone(),
            );
            let tick_result = match (mode, tick_id) {
                (RoleTickMode::Normal, Some(tick_id)) => {
                    worker.tick_with_observability_tick_id(now, tick_id).await
                }
                (RoleTickMode::Normal, None) => worker.tick(now).await,
                (RoleTickMode::Wake, Some(tick_id)) => {
                    worker
                        .tick_wake_with_observability_tick_id(now, tick_id)
                        .await
                }
                (RoleTickMode::Wake, None) => worker.tick_wake(now).await,
                (RoleTickMode::Audit, Some(tick_id)) => {
                    worker
                        .tick_audit_with_observability_tick_id(now, tick_id)
                        .await
                }
                (RoleTickMode::Audit, None) => worker.tick_audit(now).await,
            };
            match tick_result {
                Ok(progress) => report.record_success(repository.clone(), progress),
                Err(error) => report.record_failure(repository.clone(), error),
            }
        }
        report
    }
}

#[derive(Clone, Copy)]
enum RoleTickMode {
    Normal,
    Wake,
    Audit,
}

#[async_trait]
impl<F: Forge + ?Sized> Worker for MultiRepoRoleWorker<'_, F> {
    async fn tick(&self, now: DateTime<Utc>) -> Result<Progress, WorkerError> {
        self.tick_report(now).await.into_worker_result()
    }

    fn name(&self) -> &str {
        &self.name
    }
}
