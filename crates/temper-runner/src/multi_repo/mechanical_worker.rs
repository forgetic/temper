//! Multi-repository wrapper over the per-repository mechanical worker.

use super::report::{MultiRepoConfigError, MultiRepoTickReport, RepositoryJournal};
use super::repository_set::{RepositorySet, RepositoryTarget, hinted_paths, path_key};
use crate::{ExternalToolExecutors, MechanicalWorker, Progress, Worker, WorkerError};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use temper_forge::{ChangeHint, ChangeKind, Forge, ItemNumber, RepositoryPath};
use temper_workflow::{
    CommandJournal, DefaultRecoveryPolicy, LeasePolicy, ReconciliationMode, RecoveryPolicy,
    ValidatedWorkflow,
};

pub struct MultiRepoMechanicalWorker<
    'a,
    F: Forge + ?Sized,
    J: CommandJournal,
    P: RecoveryPolicy + Clone + Send + Sync = DefaultRecoveryPolicy,
> {
    name: String,
    workflow: &'a ValidatedWorkflow,
    forge: &'a F,
    repositories: RepositorySet,
    mechanical_repositories: Vec<RepositoryMechanical<'a, J>>,
    lease_policy: LeasePolicy,
    policy: P,
    external_tool_executors: ExternalToolExecutors,
}

struct RepositoryMechanical<'a, J: CommandJournal> {
    target: RepositoryTarget,
    journal: &'a J,
}

impl<'a, F, J> MultiRepoMechanicalWorker<'a, F, J, DefaultRecoveryPolicy>
where
    F: Forge + ?Sized,
    J: CommandJournal,
{
    pub fn new(
        workflow: &'a ValidatedWorkflow,
        forge: &'a F,
        repositories: RepositorySet,
        journals: Vec<RepositoryJournal<'a, J>>,
        lease_policy: LeasePolicy,
    ) -> Result<Self, MultiRepoConfigError> {
        Self::with_policy(
            workflow,
            forge,
            repositories,
            journals,
            lease_policy,
            DefaultRecoveryPolicy,
        )
    }
}

impl<'a, F, J, P> MultiRepoMechanicalWorker<'a, F, J, P>
where
    F: Forge + ?Sized,
    J: CommandJournal,
    P: RecoveryPolicy + Clone + Send + Sync,
{
    pub fn with_policy(
        workflow: &'a ValidatedWorkflow,
        forge: &'a F,
        repositories: RepositorySet,
        journals: Vec<RepositoryJournal<'a, J>>,
        lease_policy: LeasePolicy,
        policy: P,
    ) -> Result<Self, MultiRepoConfigError> {
        let mut by_repo = BTreeMap::new();
        for binding in journals {
            by_repo.insert(binding.repository.clone(), binding.journal);
        }
        let mut bound = Vec::new();
        for target in repositories.repositories() {
            let Some(journal) = by_repo.get(&target.id).copied() else {
                return Err(MultiRepoConfigError::MissingJournal {
                    repository: target.clone(),
                });
            };
            bound.push(RepositoryMechanical {
                target: target.clone(),
                journal,
            });
        }
        Ok(Self {
            name: "multi-mechanical".to_string(),
            workflow,
            forge,
            repositories,
            mechanical_repositories: bound,
            lease_policy,
            policy,
            external_tool_executors: ExternalToolExecutors::new(),
        })
    }

    /// Binds workspace executors the per-repo mechanical workers can invoke from
    /// workspace-backed queue automations. Cloned into each per-repo worker on
    /// every tick.
    pub fn with_external_tool_executors(mut self, executors: ExternalToolExecutors) -> Self {
        self.external_tool_executors = executors;
        self
    }

    pub fn repositories(&self) -> &RepositorySet {
        &self.repositories
    }

    pub async fn tick_report(&self, now: DateTime<Utc>) -> MultiRepoTickReport {
        let repositories = self.mechanical_repositories.iter().collect();
        self.tick_mechanical_repositories(now, repositories, ReconciliationMode::Bounded)
            .await
    }

    pub async fn tick_deep_audit_report(&self, now: DateTime<Utc>) -> MultiRepoTickReport {
        let repositories = self.mechanical_repositories.iter().collect();
        self.tick_mechanical_repositories(now, repositories, ReconciliationMode::DeepAudit)
            .await
    }

    pub async fn tick_hinted(
        &self,
        now: DateTime<Utc>,
        hints: &[ChangeHint],
    ) -> MultiRepoTickReport {
        let hinted = hinted_paths(hints);
        let mut repositories = Vec::with_capacity(self.mechanical_repositories.len());
        for repository in &self.mechanical_repositories {
            if hinted.contains(&path_key(&repository.target.path)) {
                repositories.push(repository);
            }
        }
        for repository in &self.mechanical_repositories {
            if !hinted.contains(&path_key(&repository.target.path)) {
                repositories.push(repository);
            }
        }
        self.tick_mechanical_repositories(now, repositories, ReconciliationMode::Bounded)
            .await
    }

    /// Ticks only repositories matching known repo hints.
    pub async fn tick_matching_hints(
        &self,
        now: DateTime<Utc>,
        hints: &[ChangeHint],
    ) -> MultiRepoTickReport {
        let hinted = hinted_paths(hints);
        let repositories = self
            .mechanical_repositories
            .iter()
            .filter(|repository| hinted.contains(&path_key(&repository.target.path)))
            .collect();
        self.tick_mechanical_repositories(now, repositories, ReconciliationMode::Bounded)
            .await
    }

    pub async fn tick_targeted(
        &self,
        now: DateTime<Utc>,
        targets: &[(RepositoryPath, ItemNumber, ChangeKind)],
    ) -> MultiRepoTickReport {
        let mut report = MultiRepoTickReport::default();
        for repository in &self.mechanical_repositories {
            let mut repo_targets: Vec<_> = targets
                .iter()
                .filter(|(path, _, _)| path_key(path) == path_key(&repository.target.path))
                .map(|(_, item, kind)| (*item, *kind))
                .collect();
            repo_targets.sort();
            repo_targets.dedup();
            if repo_targets.is_empty() {
                continue;
            }
            report.record_attempt(&repository.target);
            let worker = MechanicalWorker::with_policy(
                self.workflow,
                self.forge,
                &repository.target.id,
                repository.journal,
                self.lease_policy,
                self.policy.clone(),
            )
            .with_external_tool_executors(self.external_tool_executors.clone());
            let mut progress = Progress::unchanged();
            let mut failure = None;
            for (item, kind) in repo_targets {
                match worker.tick_artifact(now, item, kind).await {
                    Ok(item_progress) => {
                        progress.changed |= item_progress.changed;
                        progress.actions = progress.actions.saturating_add(item_progress.actions);
                    }
                    Err(error) => {
                        failure = Some(error);
                        break;
                    }
                }
            }
            if let Some(error) = failure {
                report.record_failure(repository.target.clone(), error);
            } else {
                report.record_success(repository.target.clone(), progress);
            }
        }
        report
    }

    async fn tick_mechanical_repositories(
        &self,
        now: DateTime<Utc>,
        repositories: Vec<&RepositoryMechanical<'a, J>>,
        mode: ReconciliationMode,
    ) -> MultiRepoTickReport {
        let mut report = MultiRepoTickReport::default();
        for repository in repositories {
            report.record_attempt(&repository.target);
            let worker = MechanicalWorker::with_policy(
                self.workflow,
                self.forge,
                &repository.target.id,
                repository.journal,
                self.lease_policy,
                self.policy.clone(),
            )
            .with_external_tool_executors(self.external_tool_executors.clone());
            let tick = match mode {
                ReconciliationMode::Bounded => worker.tick(now).await,
                ReconciliationMode::DeepAudit => worker.tick_deep_audit(now).await,
            };
            match tick {
                Ok(progress) => report.record_success(repository.target.clone(), progress),
                Err(error) => report.record_failure(repository.target.clone(), error),
            }
        }
        report
    }
}

#[async_trait]
impl<F, J, P> Worker for MultiRepoMechanicalWorker<'_, F, J, P>
where
    F: Forge + ?Sized,
    J: CommandJournal,
    P: RecoveryPolicy + Clone + Send + Sync,
{
    async fn tick(&self, now: DateTime<Utc>) -> Result<Progress, WorkerError> {
        self.tick_report(now).await.into_worker_result()
    }

    fn name(&self) -> &str {
        &self.name
    }
}
