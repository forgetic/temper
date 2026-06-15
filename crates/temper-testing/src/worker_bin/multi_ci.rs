//! Multi-repository fake CI producer for the testing worker.
//!
//! Production CI comes from the provider. This helper is only for the filesystem
//! process rehearsal where the fake CI worker must scan the same repository set
//! as the fixed role/mechanical worker pool.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use std::sync::Mutex;
use temper_forge_filesystem::FilesystemForge;
use temper_forge_model::{
    CiJob, CiJobConclusion, CiJobQuery, CiJobStatus, Forge, ItemNumber, PullRequest,
    PullRequestQuery, PullRequestState, RepositoryId,
};
use temper_runner::{CiSink, Progress, Worker, WorkerError};
use temper_workflow::CiStatus;

use crate::ci::FilesystemCiSink;
use crate::worker_bin::args::CiPolicyKind;

pub(super) struct MultiRepoCiWorker<'a> {
    forge: &'a FilesystemForge,
    repos: Vec<RepositoryId>,
    policy: CiPolicyKind,
    visits: Mutex<BTreeMap<(RepositoryId, ItemNumber), u64>>,
}

impl<'a> MultiRepoCiWorker<'a> {
    pub(super) fn new(
        forge: &'a FilesystemForge,
        repos: Vec<RepositoryId>,
        policy: CiPolicyKind,
    ) -> Self {
        Self {
            forge,
            repos,
            policy,
            visits: Mutex::new(BTreeMap::new()),
        }
    }

    async fn completed_jobs(
        &self,
        repo: &RepositoryId,
        pull_request: &PullRequest,
    ) -> Result<Vec<CiJob>, WorkerError> {
        Ok(self
            .forge
            .list_ci_jobs(
                repo,
                CiJobQuery {
                    pull_request_id: Some(pull_request.id.clone()),
                    commit_sha: pull_request.head_sha.clone(),
                    status: Some(CiJobStatus::Completed),
                    ..CiJobQuery::default()
                },
            )
            .await?)
    }

    fn should_record(&self, jobs: &[CiJob]) -> bool {
        match self.policy {
            CiPolicyKind::Pass | CiPolicyKind::FixedFail => jobs.is_empty(),
            CiPolicyKind::FailThenPass => jobs.is_empty() || CiStatus::from_jobs(jobs).is_failed(),
        }
    }

    fn next_visit(&self, repo: &RepositoryId, number: ItemNumber) -> u64 {
        let mut visits = self.visits.lock().expect("CI visit mutex is poisoned");
        let visit = visits.entry((repo.clone(), number)).or_insert(0);
        *visit = visit.saturating_add(1);
        *visit
    }

    fn conclusion(&self, visit: u64) -> CiJobConclusion {
        match self.policy {
            CiPolicyKind::Pass => CiJobConclusion::Success,
            CiPolicyKind::FixedFail => CiJobConclusion::Failure,
            CiPolicyKind::FailThenPass if visit == 1 => CiJobConclusion::Failure,
            CiPolicyKind::FailThenPass => CiJobConclusion::Success,
        }
    }
}

#[async_trait]
impl Worker for MultiRepoCiWorker<'_> {
    async fn tick(&self, _now: DateTime<Utc>) -> Result<Progress, WorkerError> {
        let sink = FilesystemCiSink::new(self.forge.clone());
        let mut progress = Progress::unchanged();
        for repo in &self.repos {
            let pull_requests = self
                .forge
                .list_pull_requests(
                    repo,
                    PullRequestQuery {
                        state: Some(PullRequestState::Open),
                        labels: vec!["implementation".to_string()],
                        ..PullRequestQuery::default()
                    },
                )
                .await?;
            for pull_request in pull_requests {
                let jobs = self.completed_jobs(repo, &pull_request).await?;
                if !self.should_record(&jobs) {
                    continue;
                }
                let visit = self.next_visit(repo, pull_request.number);
                sink.record(repo, pull_request.number, self.conclusion(visit))
                    .await?;
                progress.record(true);
            }
        }
        Ok(progress)
    }

    fn name(&self) -> &str {
        "ci"
    }
}
