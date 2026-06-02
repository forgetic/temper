//! Multi-repository runner wrappers.
//!
//! The existing [`RoleWorker`](crate::RoleWorker) and
//! [`MechanicalWorker`](crate::MechanicalWorker) remain the unit of actual
//! workflow behavior. This module adds a thin repository-set layer that orders a
//! configured set deterministically, records repository identity in reports, and
//! keeps ticking the remaining repositories when one repository fails.

use crate::{Agent, MechanicalWorker, Progress, RoleWorker, Worker, WorkerError};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::sync::Arc;
use temper_forge::{ChangeHint, Forge, ForgeError, RepositoryId, RepositoryPath};
use temper_workflow::{
    CommandJournal, CompiledWorkflow, DefaultRecoveryPolicy, ExecutionContext, LeasePolicy,
    RecoveryPolicy, RoleId, ValidatedWorkflow,
};

/// A repository a multi-repo worker may scan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryTarget {
    /// Stable backend repository id used for all Forge calls.
    pub id: RepositoryId,
    /// Human-facing owner/name used in reports and hint matching.
    pub path: RepositoryPath,
}

impl RepositoryTarget {
    /// Creates a repository target from its stable id and display path.
    pub fn new(id: RepositoryId, path: RepositoryPath) -> Self {
        Self { id, path }
    }

    /// Returns `owner/name` for logs, errors, and assertions.
    pub fn display_path(&self) -> String {
        format!("{}/{}", self.path.owner, self.path.name)
    }

    fn order_key(&self) -> (&str, &str, &RepositoryId) {
        (&self.path.owner, &self.path.name, &self.id)
    }
}

/// Deterministic set of repositories assigned to one worker process.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RepositorySet {
    repositories: Vec<RepositoryTarget>,
}

impl RepositorySet {
    /// Creates a set, sorted by owner/name and then id. Duplicate ids are kept
    /// only once so a worker cannot accidentally scan the same repository twice.
    pub fn new(mut repositories: Vec<RepositoryTarget>) -> Self {
        repositories.sort_by(|left, right| left.order_key().cmp(&right.order_key()));
        let mut seen = BTreeSet::new();
        repositories.retain(|repo| seen.insert(repo.id.clone()));
        Self { repositories }
    }

    /// Resolves repository ids to display paths through the portable Forge API.
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

    /// Returns repositories in deterministic scan order.
    pub fn repositories(&self) -> &[RepositoryTarget] {
        &self.repositories
    }

    /// Returns repositories named by hints, in deterministic set order.
    ///
    /// This is a narrowing helper for callers that want an early hinted pass.
    /// Hints remain advisory: callers should still run a full scan as their
    /// polling/liveness backstop.
    pub fn matching_hints<'a>(&'a self, hints: &[ChangeHint]) -> Vec<&'a RepositoryTarget> {
        let hinted = hinted_paths(hints);
        self.repositories
            .iter()
            .filter(|repo| hinted.contains(&path_key(&repo.path)))
            .collect()
    }

    /// Returns a full scan order with hinted repositories first.
    ///
    /// No repository is dropped. This lets a worker react to repo-specific hints
    /// without making them authoritative; stale, missing, or duplicated hints
    /// only affect ordering of the next broad scan.
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

/// Per-repository progress from a multi-repo tick.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryProgress {
    /// Repository that was ticked.
    pub repository: RepositoryTarget,
    /// Progress returned by that repository's per-repo worker.
    pub progress: Progress,
}

/// A repository failure captured after the wrapper continued scanning.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryFailure {
    /// Repository that failed.
    pub repository: RepositoryTarget,
    /// Display form of the per-repo worker error.
    pub message: String,
}

/// Report for one multi-repo worker tick.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MultiRepoTickReport {
    /// Combined progress across successful repositories.
    pub progress: Progress,
    /// Per-repository successes in attempted scan order.
    pub repositories: Vec<RepositoryProgress>,
    /// Per-repository failures in attempted scan order.
    pub failures: Vec<RepositoryFailure>,
}

impl MultiRepoTickReport {
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

    /// Converts the report into the [`Worker`] trait result shape.
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

/// Error returned by a multi-repo worker after it has attempted every repo.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MultiRepoError {
    /// Combined progress made by repositories that did not fail.
    pub progress: Progress,
    /// Repository-scoped failures.
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

/// Per-repository journal binding for a multi-repo mechanical worker.
#[derive(Clone, Copy)]
pub struct RepositoryJournal<'a, J: CommandJournal> {
    /// Repository whose recovery commands are stored in `journal`.
    pub repository: &'a RepositoryId,
    /// Journal dedicated to this repository.
    pub journal: &'a J,
}

/// Configuration errors building a multi-repo worker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MultiRepoConfigError {
    /// The mechanical worker needs one journal per repository.
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

/// Role worker wrapper for a deterministic repository set.
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
    /// Creates a multi-repo role worker with the default `multi-role:<id>` name.
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

    /// Configured repository set.
    pub fn repositories(&self) -> &RepositorySet {
        &self.repositories
    }

    /// Ticks every repository in deterministic order, returning a partial
    /// report instead of stopping at the first failure.
    pub async fn tick_report(&self, now: DateTime<Utc>) -> MultiRepoTickReport {
        self.tick_repositories(now, self.repositories.iter_refs())
            .await
    }

    /// Ticks every repository with hinted repositories first.
    pub async fn tick_hinted(
        &self,
        now: DateTime<Utc>,
        hints: &[ChangeHint],
    ) -> MultiRepoTickReport {
        self.tick_repositories(now, self.repositories.hinted_order(hints))
            .await
    }

    async fn tick_repositories(
        &self,
        now: DateTime<Utc>,
        repositories: Vec<&RepositoryTarget>,
    ) -> MultiRepoTickReport {
        let mut report = MultiRepoTickReport::default();
        for repository in repositories {
            let worker = RoleWorker::new(
                self.workflow,
                self.compiled,
                self.forge,
                &repository.id,
                self.role.clone(),
                Arc::clone(&self.agent),
                self.context.clone(),
            );
            match worker.tick(now).await {
                Ok(progress) => report.record_success(repository.clone(), progress),
                Err(error) => report.record_failure(repository.clone(), error),
            }
        }
        report
    }
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

/// Mechanical worker wrapper for a deterministic repository set.
pub struct MultiRepoMechanicalWorker<
    'a,
    F: Forge + ?Sized,
    J: CommandJournal,
    P: RecoveryPolicy + Clone + Send + Sync = DefaultRecoveryPolicy,
> {
    name: String,
    workflow: &'a ValidatedWorkflow,
    forge: &'a F,
    repositories: Vec<RepositoryMechanical<'a, J>>,
    lease_policy: LeasePolicy,
    policy: P,
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
    /// Creates a multi-repo mechanical worker using [`DefaultRecoveryPolicy`].
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
    /// Creates a multi-repo mechanical worker with an injectable policy.
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
            repositories: bound,
            lease_policy,
            policy,
        })
    }

    /// Ticks every repository in deterministic order, returning partial results.
    pub async fn tick_report(&self, now: DateTime<Utc>) -> MultiRepoTickReport {
        let mut report = MultiRepoTickReport::default();
        for repository in &self.repositories {
            let worker = MechanicalWorker::with_policy(
                self.workflow,
                self.forge,
                &repository.target.id,
                repository.journal,
                self.lease_policy,
                self.policy.clone(),
            );
            match worker.tick(now).await {
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
