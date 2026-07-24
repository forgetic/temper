use crate::ids::{CiJobId, PullRequestId, RepositoryId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Execution status for a CI job.
#[derive(Copy, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CiJobStatus {
    Queued,
    Running,
    Completed,
}

/// Terminal result for a completed CI job.
#[derive(Copy, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CiJobConclusion {
    Success,
    Failure,
    Cancelled,
    Skipped,
    TimedOut,
    Neutral,
}

/// CI job associated with a commit and optionally a pull request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CiJob {
    pub id: CiJobId,
    pub repo_id: RepositoryId,
    pub pull_request_id: Option<PullRequestId>,
    pub commit_sha: String,
    pub name: String,
    pub status: CiJobStatus,
    pub conclusion: Option<CiJobConclusion>,
    pub url: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

/// CI jobs plus evidence that the query matched a provider CI execution.
///
/// A provider may register a workflow run before assigning any of its jobs to a
/// runner. In that state `jobs` is empty while `matching_ci_present` is true.
/// Keeping those facts separate prevents ordinary runner queueing from looking
/// like a missing current-head CI run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CiJobListing {
    jobs: Vec<CiJob>,
    matching_ci_present: bool,
}

impl CiJobListing {
    /// Builds a listing from provider evidence.
    ///
    /// A non-empty job set always proves matching CI presence even if a backend
    /// supplies `false` defensively.
    pub fn new(jobs: Vec<CiJob>, matching_ci_present: bool) -> Self {
        Self {
            matching_ci_present: matching_ci_present || !jobs.is_empty(),
            jobs,
        }
    }

    /// Builds a listing for backends whose only CI records are jobs.
    pub fn from_jobs(jobs: Vec<CiJob>) -> Self {
        Self::new(jobs, false)
    }

    /// Returns the filtered, deterministically ordered jobs.
    pub fn jobs(&self) -> &[CiJob] {
        &self.jobs
    }

    /// Consumes the listing and returns its jobs.
    pub fn into_jobs(self) -> Vec<CiJob> {
        self.jobs
    }

    /// Whether provider evidence matched the query's PR/commit ownership scope.
    pub fn matching_ci_present(&self) -> bool {
        self.matching_ci_present
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonempty_jobs_always_imply_matching_ci_presence() {
        let job = CiJob {
            id: CiJobId::new("job-1"),
            repo_id: RepositoryId::new("repo-1"),
            pull_request_id: None,
            commit_sha: "head-1".into(),
            name: "test".into(),
            status: CiJobStatus::Queued,
            conclusion: None,
            url: None,
            created_at: DateTime::UNIX_EPOCH,
            started_at: None,
            completed_at: None,
            updated_at: DateTime::UNIX_EPOCH,
        };
        let listing = CiJobListing::new(vec![job], false);
        assert!(listing.matching_ci_present());
        assert_eq!(listing.jobs().len(), 1);
    }

    #[test]
    fn matching_ci_can_exist_before_jobs_materialize() {
        let listing = CiJobListing::new(Vec::new(), true);
        assert!(listing.matching_ci_present());
        assert!(listing.jobs().is_empty());
    }
}
