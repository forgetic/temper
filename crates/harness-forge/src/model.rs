use crate::ids::{
    CiJobId, CommentId, IssueId, ItemNumber, LabelId, PullRequestId, RepositoryId, UserId,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// User account known to a Forge backend.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct User {
    pub id: UserId,
    pub handle: String,
    pub display_name: Option<String>,
    pub email: Option<String>,
}

/// Human-facing owner/name repository lookup key.
///
/// A repository path is convenient for user input and provider URLs, but it is
/// not stable identity. Store `RepositoryId` for durable synchronization.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RepositoryPath {
    pub owner: String,
    pub name: String,
}

impl RepositoryPath {
    /// Creates a repository path from owner and repository name values.
    pub fn new(owner: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            owner: owner.into(),
            name: name.into(),
        }
    }
}

/// Repository containing source code and collaboration artifacts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Repository {
    pub id: RepositoryId,
    pub owner: String,
    pub name: String,
    pub default_branch: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Input used to create a repository.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CreateRepository {
    pub owner: String,
    pub name: String,
    pub default_branch: String,
    pub description: Option<String>,
}

/// Label metadata scoped to a repository.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Label {
    pub id: LabelId,
    pub repo_id: RepositoryId,
    pub name: String,
    pub color: Option<String>,
    pub description: Option<String>,
}

/// Input used to create or update a label.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UpsertLabel {
    pub name: String,
    pub color: Option<String>,
    pub description: Option<String>,
}

/// Comment on an issue or pull request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Comment {
    pub id: CommentId,
    pub author_id: UserId,
    pub body: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Input used to add a comment.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CreateComment {
    pub body: String,
}

/// Lifecycle state for an issue.
#[derive(Copy, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueState {
    Open,
    Closed,
}

/// Issue tracked by a repository.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Issue {
    pub id: IssueId,
    pub repo_id: RepositoryId,
    pub number: ItemNumber,
    pub title: String,
    pub body: String,
    pub state: IssueState,
    pub author_id: UserId,
    pub labels: Vec<String>,
    pub assignees: Vec<UserId>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

/// Input used to create an issue.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CreateIssue {
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub assignees: Vec<UserId>,
}

/// Partial issue update.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct UpdateIssue {
    pub title: Option<String>,
    pub body: Option<String>,
    pub state: Option<IssueState>,
    pub set_labels: Option<Vec<String>>,
    pub add_labels: Vec<String>,
    pub remove_labels: Vec<String>,
    pub add_assignees: Vec<UserId>,
    pub remove_assignees: Vec<UserId>,
}

/// Reference to a branch in a repository.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BranchRef {
    pub repository_id: RepositoryId,
    pub branch: String,
}

/// Lifecycle state for a pull request.
#[derive(Copy, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PullRequestState {
    Open,
    Closed,
    Merged,
}

/// State transition requested through a pull-request update.
///
/// `Merged` is intentionally absent. Use `Forge::merge_pull_request` to merge
/// a pull request so the backend can also produce a `MergeRecord`.
#[derive(Copy, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PullRequestUpdateState {
    Open,
    Closed,
}

impl From<PullRequestUpdateState> for PullRequestState {
    fn from(state: PullRequestUpdateState) -> Self {
        match state {
            PullRequestUpdateState::Open => Self::Open,
            PullRequestUpdateState::Closed => Self::Closed,
        }
    }
}

/// Method used to merge a pull request.
#[derive(Copy, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeMethod {
    MergeCommit,
    Squash,
    Rebase,
}

/// Recorded merge of a pull request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MergeRecord {
    pub method: MergeMethod,
    pub commit_sha: String,
    pub merged_by: UserId,
    pub merged_at: DateTime<Utc>,
}

/// Pull request proposing changes from one branch to another.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PullRequest {
    pub id: PullRequestId,
    pub repo_id: RepositoryId,
    pub number: ItemNumber,
    pub title: String,
    pub body: String,
    pub state: PullRequestState,
    pub author_id: UserId,
    pub source: BranchRef,
    pub target: BranchRef,
    pub head_sha: Option<String>,
    pub base_sha: Option<String>,
    pub labels: Vec<String>,
    pub assignees: Vec<UserId>,
    pub merge: Option<MergeRecord>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

/// Input used to create a pull request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CreatePullRequest {
    pub title: String,
    pub body: String,
    pub source: BranchRef,
    pub target: BranchRef,
    pub labels: Vec<String>,
    pub assignees: Vec<UserId>,
}

/// Partial pull-request update.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct UpdatePullRequest {
    pub title: Option<String>,
    pub body: Option<String>,
    pub state: Option<PullRequestUpdateState>,
    pub set_labels: Option<Vec<String>>,
    pub add_labels: Vec<String>,
    pub remove_labels: Vec<String>,
    pub add_assignees: Vec<UserId>,
    pub remove_assignees: Vec<UserId>,
}

/// Input used to merge a pull request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MergePullRequest {
    pub method: MergeMethod,
    pub commit_title: Option<String>,
    pub commit_body: Option<String>,
}

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
