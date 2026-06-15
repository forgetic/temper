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
