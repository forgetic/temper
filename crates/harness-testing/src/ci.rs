use chrono::Duration;
use harness_forge::{
    CiJob, CiJobConclusion, CiJobId, CiJobQuery, CiJobStatus, Forge, ForgeError, ItemNumber,
    PullRequest, RepositoryId,
};
use harness_forge_filesystem::FilesystemForge;
use harness_forge_memory::MemoryForge;
use harness_runner::{CiError, CiPolicy, CiSink};
use harness_workflow::CiStatus;

use crate::ts;

#[derive(Clone)]
pub struct MemoryCiSink {
    forge: MemoryForge,
}

impl MemoryCiSink {
    pub fn new(forge: MemoryForge) -> Self {
        Self { forge }
    }
}

#[async_trait::async_trait]
impl CiSink for MemoryCiSink {
    async fn record(
        &self,
        repo: &RepositoryId,
        pr: ItemNumber,
        conclusion: CiJobConclusion,
    ) -> Result<(), CiError> {
        let pull_request = self
            .forge
            .get_pull_request_by_number(repo, pr)
            .await?
            .ok_or_else(|| ForgeError::NotFound(format!("pull request #{pr}")))?;
        let mut jobs = self.forge.list_ci_jobs(repo, CiJobQuery::default()).await?;
        let next = jobs.len().saturating_add(1);
        jobs.push(ci_job(repo, &pull_request, next, conclusion));
        self.forge.seed_ci_jobs(repo, jobs);
        Ok(())
    }
}

#[derive(Clone)]
pub struct FilesystemCiSink {
    forge: FilesystemForge,
}

impl FilesystemCiSink {
    pub fn new(forge: FilesystemForge) -> Self {
        Self { forge }
    }
}

#[async_trait::async_trait]
impl CiSink for FilesystemCiSink {
    async fn record(
        &self,
        repo: &RepositoryId,
        pr: ItemNumber,
        conclusion: CiJobConclusion,
    ) -> Result<(), CiError> {
        let pull_request = self
            .forge
            .get_pull_request_by_number(repo, pr)
            .await?
            .ok_or_else(|| ForgeError::NotFound(format!("pull request #{pr}")))?;
        let mut jobs = self.forge.list_ci_jobs(repo, CiJobQuery::default()).await?;
        let next = jobs.len().saturating_add(1);
        jobs.push(ci_job(repo, &pull_request, next, conclusion));
        self.forge.seed_ci_jobs(repo, jobs)?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FixedCiPolicy {
    conclusion: CiJobConclusion,
}

impl FixedCiPolicy {
    pub fn pass() -> Self {
        Self {
            conclusion: CiJobConclusion::Success,
        }
    }

    pub fn fail() -> Self {
        Self {
            conclusion: CiJobConclusion::Failure,
        }
    }
}

impl CiPolicy for FixedCiPolicy {
    fn conclusion(&self, _pull_request: &PullRequest, _visit: u64) -> CiJobConclusion {
        self.conclusion
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FailThenPassCiPolicy;

impl CiPolicy for FailThenPassCiPolicy {
    fn should_record(&self, _pull_request: &PullRequest, jobs: &[CiJob]) -> bool {
        jobs.is_empty() || CiStatus::from_jobs(jobs).is_failed()
    }

    fn conclusion(&self, _pull_request: &PullRequest, visit: u64) -> CiJobConclusion {
        if visit == 1 {
            CiJobConclusion::Failure
        } else {
            CiJobConclusion::Success
        }
    }
}

fn ci_job(
    repo: &RepositoryId,
    pull_request: &PullRequest,
    index: usize,
    conclusion: CiJobConclusion,
) -> CiJob {
    let offset = i64::try_from(index).unwrap_or(i64::MAX);
    let created_at = ts("2026-05-29T00:00:00Z") + Duration::seconds(offset);
    CiJob {
        id: CiJobId::new(format!(
            "ci-{}-{}-{index}",
            repo.as_str(),
            pull_request.number.get()
        )),
        repo_id: repo.clone(),
        pull_request_id: Some(pull_request.id.clone()),
        commit_sha: pull_request
            .head_sha
            .clone()
            .unwrap_or_else(|| format!("pr-{}-head", pull_request.number.get())),
        name: "ci".into(),
        status: CiJobStatus::Completed,
        conclusion: Some(conclusion),
        url: None,
        created_at,
        started_at: Some(created_at + Duration::seconds(30)),
        completed_at: Some(created_at + Duration::seconds(60)),
        updated_at: created_at + Duration::seconds(60),
    }
}
