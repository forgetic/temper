use crate::ids::{CiJobId, IssueId, PullRequestId, RepositoryId, UserId};
use crate::model::{
    CiJob, CiJobStatus, Comment, CreateComment, CreateIssue, CreatePullRequest, CreateRepository,
    Issue, IssueState, Label, MergePullRequest, MergeRecord, PullRequest, PullRequestState,
    Repository, UpdateIssue, UpdatePullRequest, UpsertLabel, User,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Result type returned by Forge operations.
pub type ForgeResult<T> = Result<T, ForgeError>;

/// Portable error categories for Forge backends.
#[derive(Debug, Error)]
pub enum ForgeError {
    #[error("resource not found: {0}")]
    NotFound(String),

    #[error("resource already exists: {0}")]
    AlreadyExists(String),

    #[error("invalid request: {0}")]
    InvalidRequest(String),

    #[error("operation conflict: {0}")]
    Conflict(String),

    #[error("backend error: {0}")]
    Backend(String),
}

/// Issue listing filter.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct IssueQuery {
    pub state: Option<IssueState>,
    pub labels: Vec<String>,
    pub author_id: Option<UserId>,
    pub assignee_id: Option<UserId>,
}

/// Pull-request listing filter.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PullRequestQuery {
    pub state: Option<PullRequestState>,
    pub labels: Vec<String>,
    pub author_id: Option<UserId>,
    pub assignee_id: Option<UserId>,
}

/// CI job listing filter.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CiJobQuery {
    pub pull_request_id: Option<PullRequestId>,
    pub commit_sha: Option<String>,
    pub status: Option<CiJobStatus>,
}

/// Backend-agnostic interface for Forge-like collaboration systems.
///
/// Implementations adapt this trait to a concrete backend such as a local
/// filesystem store, Forgejo, GitHub, or a test double. Methods return portable
/// domain types and errors so workflow logic can be written once and reused
/// across backends.
#[async_trait]
pub trait Forge: Send + Sync {
    /// Returns the user identity used by this backend client.
    async fn current_user(&self) -> ForgeResult<User>;

    /// Looks up a user by stable backend identifier.
    async fn get_user(&self, id: &UserId) -> ForgeResult<Option<User>>;

    /// Lists repositories visible to the backend client.
    async fn list_repositories(&self) -> ForgeResult<Vec<Repository>>;

    /// Creates a repository.
    async fn create_repository(&self, input: CreateRepository) -> ForgeResult<Repository>;

    /// Looks up a repository by stable backend identifier.
    async fn get_repository(&self, id: &RepositoryId) -> ForgeResult<Option<Repository>>;

    /// Lists labels in a repository.
    async fn list_labels(&self, repo_id: &RepositoryId) -> ForgeResult<Vec<Label>>;

    /// Creates or updates a repository label by name.
    async fn upsert_label(&self, repo_id: &RepositoryId, input: UpsertLabel) -> ForgeResult<Label>;

    /// Lists issues in a repository.
    async fn list_issues(
        &self,
        repo_id: &RepositoryId,
        query: IssueQuery,
    ) -> ForgeResult<Vec<Issue>>;

    /// Creates an issue in a repository.
    async fn create_issue(&self, repo_id: &RepositoryId, input: CreateIssue) -> ForgeResult<Issue>;

    /// Looks up an issue by stable backend identifier.
    async fn get_issue(&self, id: &IssueId) -> ForgeResult<Option<Issue>>;

    /// Updates an issue.
    async fn update_issue(&self, id: &IssueId, input: UpdateIssue) -> ForgeResult<Issue>;

    /// Lists comments on an issue.
    async fn list_issue_comments(&self, id: &IssueId) -> ForgeResult<Vec<Comment>>;

    /// Adds a comment to an issue.
    async fn add_issue_comment(&self, id: &IssueId, input: CreateComment) -> ForgeResult<Comment>;

    /// Lists pull requests in a repository.
    async fn list_pull_requests(
        &self,
        repo_id: &RepositoryId,
        query: PullRequestQuery,
    ) -> ForgeResult<Vec<PullRequest>>;

    /// Creates a pull request in a repository.
    async fn create_pull_request(
        &self,
        repo_id: &RepositoryId,
        input: CreatePullRequest,
    ) -> ForgeResult<PullRequest>;

    /// Looks up a pull request by stable backend identifier.
    async fn get_pull_request(&self, id: &PullRequestId) -> ForgeResult<Option<PullRequest>>;

    /// Updates a pull request.
    async fn update_pull_request(
        &self,
        id: &PullRequestId,
        input: UpdatePullRequest,
    ) -> ForgeResult<PullRequest>;

    /// Lists comments on a pull request.
    async fn list_pull_request_comments(&self, id: &PullRequestId) -> ForgeResult<Vec<Comment>>;

    /// Adds a comment to a pull request.
    async fn add_pull_request_comment(
        &self,
        id: &PullRequestId,
        input: CreateComment,
    ) -> ForgeResult<Comment>;

    /// Merges a pull request.
    async fn merge_pull_request(
        &self,
        id: &PullRequestId,
        input: MergePullRequest,
    ) -> ForgeResult<MergeRecord>;

    /// Lists CI jobs in a repository.
    async fn list_ci_jobs(
        &self,
        repo_id: &RepositoryId,
        query: CiJobQuery,
    ) -> ForgeResult<Vec<CiJob>>;

    /// Looks up a CI job by stable backend identifier.
    async fn get_ci_job(&self, id: &CiJobId) -> ForgeResult<Option<CiJob>>;
}
