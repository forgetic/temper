//! Test-only outside-world signal producers.
//!
//! The workflow engine reads CI natively through
//! [`Forge::list_ci_jobs`](temper_forge_model::Forge::list_ci_jobs); a production
//! deployment relies on its provider (for example Forgejo Actions) to create
//! those jobs. The seam in this module exists so layered tests can plug in a
//! fake producer without teaching the workflow engine or role agents how to
//! mutate CI state.

use crate::{Progress, Worker, WorkerError};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::sync::Mutex;
use temper_forge_model::{
    CiJob, CiJobConclusion, CiJobQuery, CiJobStatus, Forge, ForgeError, ItemNumber, PullRequest,
    PullRequestQuery, PullRequestState, RepositoryId,
};

/// Error returned by a fake CI producer.
#[derive(Debug)]
pub enum CiError {
    /// Reading or writing through a Forge backend failed.
    Forge(ForgeError),
    /// Producer-specific failure.
    Message(String),
}

impl CiError {
    /// Creates a producer error from a displayable message.
    pub fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }
}

impl fmt::Display for CiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CiError::Forge(error) => {
                write!(formatter, "CI producer forge operation failed: {error}")
            }
            CiError::Message(message) => formatter.write_str(message),
        }
    }
}

impl Error for CiError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            CiError::Forge(error) => Some(error),
            CiError::Message(_) => None,
        }
    }
}

impl From<ForgeError> for CiError {
    fn from(error: ForgeError) -> Self {
        Self::Forge(error)
    }
}

/// Test-side sink that records a native CI conclusion for a pull request.
///
/// Production does not implement this trait: the provider's real CI system is
/// the producer, and the engine already observes its jobs through the portable
/// Forge API.
#[async_trait]
pub trait CiSink: Send + Sync {
    /// Records a completed CI verdict for `pr` in `repo`.
    async fn record(
        &self,
        repo: &RepositoryId,
        pr: ItemNumber,
        conclusion: CiJobConclusion,
    ) -> Result<(), CiError>;
}

/// Policy deciding which CI conclusion a fake producer should record.
pub trait CiPolicy: Send + Sync {
    /// Returns whether the fake producer should record a new verdict for this
    /// pull request given the completed jobs already present for its current
    /// head. The default records exactly one verdict per head.
    fn should_record(&self, _pull_request: &PullRequest, jobs: &[CiJob]) -> bool {
        jobs.is_empty()
    }

    /// Returns the conclusion to record for `pull_request` on this producer
    /// visit. `visit` is one-based and increments only when the worker is about
    /// to record a job for the pull request.
    fn conclusion(&self, pull_request: &PullRequest, visit: u64) -> CiJobConclusion;
}

impl<T> CiPolicy for T
where
    T: Fn(&PullRequest, u64) -> CiJobConclusion + Send + Sync,
{
    fn conclusion(&self, pull_request: &PullRequest, visit: u64) -> CiJobConclusion {
        self(pull_request, visit)
    }
}

/// Default CI policy: every visited pull request passes.
#[derive(Clone, Copy, Debug, Default)]
pub struct PassCiPolicy;

impl CiPolicy for PassCiPolicy {
    fn conclusion(&self, _pull_request: &PullRequest, _visit: u64) -> CiJobConclusion {
        CiJobConclusion::Success
    }
}

/// Worker that produces fake CI verdicts for implementation pull requests.
///
/// Each tick lists open PRs labeled `implementation`, asks the injected
/// [`CiPolicy`] whether another verdict is needed for the current head, and asks
/// the injected [`CiSink`] to record the chosen conclusion. The worker is a test
/// producer only; merge gates continue to read CI through the engine's native
/// `list_ci_jobs` signal path.
pub struct CiWorker<'a, F: Forge + ?Sized, S, P = PassCiPolicy> {
    name: String,
    forge: &'a F,
    repo: &'a RepositoryId,
    sink: S,
    policy: P,
    visits: Mutex<BTreeMap<ItemNumber, u64>>,
}

impl<'a, F, S> CiWorker<'a, F, S, PassCiPolicy>
where
    F: Forge + ?Sized,
    S: CiSink,
{
    /// Creates a CI worker using the default pass policy.
    pub fn new(forge: &'a F, repo: &'a RepositoryId, sink: S) -> Self {
        Self::with_policy(forge, repo, sink, PassCiPolicy)
    }
}

impl<'a, F, S, P> CiWorker<'a, F, S, P>
where
    F: Forge + ?Sized,
    S: CiSink,
    P: CiPolicy,
{
    /// Creates a CI worker with an explicit policy.
    pub fn with_policy(forge: &'a F, repo: &'a RepositoryId, sink: S, policy: P) -> Self {
        Self {
            name: "ci".to_string(),
            forge,
            repo,
            sink,
            policy,
            visits: Mutex::new(BTreeMap::new()),
        }
    }

    async fn completed_jobs(&self, pull_request: &PullRequest) -> Result<Vec<CiJob>, WorkerError> {
        Ok(self
            .forge
            .list_ci_jobs(
                self.repo,
                CiJobQuery {
                    pull_request_id: Some(pull_request.id.clone()),
                    commit_sha: pull_request.head_sha.clone(),
                    status: Some(CiJobStatus::Completed),
                    ..CiJobQuery::default()
                },
            )
            .await?)
    }

    fn next_visit(&self, number: ItemNumber) -> u64 {
        let mut visits = self
            .visits
            .lock()
            .expect("CI worker visit mutex is poisoned");
        let visit = visits.entry(number).or_insert(0);
        *visit = visit.saturating_add(1);
        *visit
    }
}

#[async_trait]
impl<F, S, P> Worker for CiWorker<'_, F, S, P>
where
    F: Forge + ?Sized,
    S: CiSink,
    P: CiPolicy,
{
    async fn tick(&self, _now: DateTime<Utc>) -> Result<Progress, WorkerError> {
        let pull_requests = self
            .forge
            .list_pull_requests(
                self.repo,
                PullRequestQuery {
                    state: Some(PullRequestState::Open),
                    labels: vec!["implementation".to_string()],
                    ..PullRequestQuery::default()
                },
            )
            .await?;

        let mut progress = Progress::unchanged();
        for pull_request in pull_requests {
            let jobs = self.completed_jobs(&pull_request).await?;
            if !self.policy.should_record(&pull_request, &jobs) {
                continue;
            }
            let visit = self.next_visit(pull_request.number);
            let conclusion = self.policy.conclusion(&pull_request, visit);
            self.sink
                .record(self.repo, pull_request.number, conclusion)
                .await?;
            progress.record(true);
        }
        Ok(progress)
    }

    fn name(&self) -> &str {
        &self.name
    }
}
