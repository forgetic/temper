//! Multi-repository runner wrappers over per-repository role and mechanical workers.

use crate::{
    Agent, ExternalToolExecutors, MechanicalWorker, Progress, RoleWorker, Worker, WorkerError,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::sync::Arc;
use temper_forge::{
    ChangeHint, ChangeKind, Forge, ForgeError, ItemNumber, RepositoryId, RepositoryPath,
};
use temper_workflow::{
    CommandJournal, CompiledWorkflow, DefaultRecoveryPolicy, ExecutionContext, LeasePolicy,
    ReconciliationMode, RecoveryPolicy, RoleId, ValidatedWorkflow,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryTarget {
    pub id: RepositoryId,
    pub path: RepositoryPath,
}

impl RepositoryTarget {
    pub fn new(id: RepositoryId, path: RepositoryPath) -> Self {
        Self { id, path }
    }

    pub fn display_path(&self) -> String {
        format!("{}/{}", self.path.owner, self.path.name)
    }

    fn order_key(&self) -> (&str, &str, &RepositoryId) {
        (&self.path.owner, &self.path.name, &self.id)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RepositorySet {
    repositories: Vec<RepositoryTarget>,
}

impl RepositorySet {
    pub fn new(mut repositories: Vec<RepositoryTarget>) -> Self {
        repositories.sort_by(|left, right| left.order_key().cmp(&right.order_key()));
        let mut seen = BTreeSet::new();
        repositories.retain(|repo| seen.insert(repo.id.clone()));
        Self { repositories }
    }

    pub async fn resolve<F, I>(forge: &F, ids: I) -> Result<Self, ForgeError>
    where
        F: Forge + ?Sized,
        I: IntoIterator<Item = RepositoryId>,
    {
        let mut targets = Vec::new();
        for id in ids {
            let repo = forge
                .get_repository(&id)
                .await?
                .ok_or_else(|| ForgeError::NotFound(format!("repository {id}")))?;
            targets.push(RepositoryTarget::new(
                repo.id,
                RepositoryPath::new(repo.owner, repo.name),
            ));
        }
        Ok(Self::new(targets))
    }

    pub fn repositories(&self) -> &[RepositoryTarget] {
        &self.repositories
    }

    pub fn matching_hints<'a>(&'a self, hints: &[ChangeHint]) -> Vec<&'a RepositoryTarget> {
        let hinted = hinted_paths(hints);
        self.repositories
            .iter()
            .filter(|repo| hinted.contains(&path_key(&repo.path)))
            .collect()
    }

    pub fn hinted_order<'a>(&'a self, hints: &[ChangeHint]) -> Vec<&'a RepositoryTarget> {
        let hinted = hinted_paths(hints);
        let mut ordered = Vec::with_capacity(self.repositories.len());
        for repo in &self.repositories {
            if hinted.contains(&path_key(&repo.path)) {
                ordered.push(repo);
            }
        }
        for repo in &self.repositories {
            if !hinted.contains(&path_key(&repo.path)) {
                ordered.push(repo);
            }
        }
        ordered
    }

    fn iter_refs(&self) -> Vec<&RepositoryTarget> {
        self.repositories.iter().collect()
    }
}

fn hinted_paths(hints: &[ChangeHint]) -> BTreeSet<(String, String)> {
    hints.iter().map(|hint| path_key(&hint.repo)).collect()
}

fn path_key(path: &RepositoryPath) -> (String, String) {
    (path.owner.clone(), path.name.clone())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryProgress {
    pub repository: RepositoryTarget,
    pub progress: Progress,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryFailure {
    pub repository: RepositoryTarget,
    pub message: String,
}

/// Report for one multi-repo tick, including scan-count diagnostics.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MultiRepoTickReport {
    /// Combined progress across successful repositories.
    pub progress: Progress,
    /// Repositories attempted in scan order, including failures.
    pub attempted_repositories: Vec<RepositoryTarget>,
    /// Per-repository successes in attempted scan order.
    pub repositories: Vec<RepositoryProgress>,
    /// Per-repository failures in attempted scan order.
    pub failures: Vec<RepositoryFailure>,
}

impl MultiRepoTickReport {
    fn record_attempt(&mut self, repository: &RepositoryTarget) {
        self.attempted_repositories.push(repository.clone());
    }

    fn record_success(&mut self, repository: RepositoryTarget, progress: Progress) {
        self.progress.changed |= progress.changed;
        self.progress.actions = self.progress.actions.saturating_add(progress.actions);
        self.repositories.push(RepositoryProgress {
            repository,
            progress,
        });
    }

    fn record_failure(&mut self, repository: RepositoryTarget, error: WorkerError) {
        self.failures.push(RepositoryFailure {
            repository,
            message: error.to_string(),
        });
    }

    /// Number of repositories this tick attempted to scan.
    pub fn scanned_repository_count(&self) -> usize {
        self.attempted_repositories.len()
    }

    /// Display paths for repositories this tick attempted to scan.
    pub fn scanned_repository_paths(&self) -> Vec<String> {
        self.attempted_repositories
            .iter()
            .map(RepositoryTarget::display_path)
            .collect()
    }

    pub fn into_worker_result(self) -> Result<Progress, WorkerError> {
        if self.failures.is_empty() {
            Ok(self.progress)
        } else {
            Err(WorkerError::MultiRepo(MultiRepoError {
                progress: self.progress,
                failures: self.failures,
            }))
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MultiRepoError {
    pub progress: Progress,
    pub failures: Vec<RepositoryFailure>,
}

impl fmt::Display for MultiRepoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} repositories failed during multi-repo tick",
            self.failures.len()
        )?;
        for failure in &self.failures {
            write!(
                formatter,
                "; {}: {}",
                failure.repository.display_path(),
                failure.message
            )?;
        }
        Ok(())
    }
}

impl Error for MultiRepoError {}

#[derive(Clone, Copy)]
pub struct RepositoryJournal<'a, J: CommandJournal> {
    pub repository: &'a RepositoryId,
    pub journal: &'a J,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MultiRepoConfigError {
    MissingJournal { repository: RepositoryTarget },
}

impl fmt::Display for MultiRepoConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingJournal { repository } => write!(
                formatter,
                "missing command journal for repository {}",
                repository.display_path()
            ),
        }
    }
}

impl Error for MultiRepoConfigError {}

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
