use crate::ids::{CiJobId, IssueId, ItemNumber, PullRequestId, RepositoryId, UserId};
use crate::model::{
    CiJob, CiJobStatus, Comment, CreateComment, CreateIssue, CreatePullRequest,
    CreatePullRequestReview, CreateRepository, Issue, IssueState, Label, MergePullRequest,
    MergeRecord, PullRequest, PullRequestReview, PullRequestState, Repository, RepositoryPath,
    RequestReviewers, UpdateIssue, UpdatePullRequest, UpsertLabel, User,
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

/// Sort direction for list operations.
#[derive(Copy, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SortDirection {
    Asc,
    Desc,
}

/// Repository field used for sorting repository lists.
#[derive(Copy, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RepositorySortField {
    Path,
    CreatedAt,
    UpdatedAt,
}

/// Sort order for repository lists.
#[derive(Copy, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RepositorySort {
    pub field: RepositorySortField,
    pub direction: SortDirection,
}

/// Repository listing query.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct RepositoryQuery {
    pub sort: Option<RepositorySort>,
}

/// Issue or pull-request field used for sorting item lists.
#[derive(Copy, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemSortField {
    Number,
    CreatedAt,
    UpdatedAt,
}

/// Sort order for issue and pull-request lists.
#[derive(Copy, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ItemSort {
    pub field: ItemSortField,
    pub direction: SortDirection,
}

/// Detail flags for issue and pull-request list results.
///
/// Defaults preserve the full Forge contract. Callers that only need summary
/// fields such as labels, body, state, and assignees may request
/// [`Self::summary`] to let backends skip expensive enrichment.
#[derive(Copy, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ItemListDetails {
    /// Whether native dependency links should be populated in list results.
    #[serde(default = "default_include_dependencies")]
    pub dependencies: bool,
}

impl ItemListDetails {
    /// Full item detail, including native dependency links.
    pub const fn full() -> Self {
        Self { dependencies: true }
    }

    /// Summary item detail, omitting native dependency-link enrichment.
    pub const fn summary() -> Self {
        Self {
            dependencies: false,
        }
    }
}

impl Default for ItemListDetails {
    fn default() -> Self {
        Self::full()
    }
}

const fn default_include_dependencies() -> bool {
    true
}

/// Issue listing query.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct IssueQuery {
    pub state: Option<IssueState>,
    pub labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_contains: Option<String>,
    pub author_id: Option<UserId>,
    pub assignee_id: Option<UserId>,
    pub sort: Option<ItemSort>,
    #[serde(default)]
    pub details: ItemListDetails,
}

/// Pull-request listing query.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PullRequestQuery {
    pub state: Option<PullRequestState>,
    pub labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_contains: Option<String>,
    pub author_id: Option<UserId>,
    pub assignee_id: Option<UserId>,
    pub sort: Option<ItemSort>,
    #[serde(default)]
    pub details: ItemListDetails,
}

/// CI job field used for sorting CI job lists.
#[derive(Copy, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CiJobSortField {
    Name,
    CreatedAt,
    UpdatedAt,
}

/// Sort order for CI job lists.
#[derive(Copy, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CiJobSort {
    pub field: CiJobSortField,
    pub direction: SortDirection,
}

/// CI job listing query.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CiJobQuery {
    pub pull_request_id: Option<PullRequestId>,
    pub commit_sha: Option<String>,
    pub status: Option<CiJobStatus>,
    pub sort: Option<CiJobSort>,
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
    async fn list_repositories(&self, query: RepositoryQuery) -> ForgeResult<Vec<Repository>>;

    /// Creates a repository.
    async fn create_repository(&self, input: CreateRepository) -> ForgeResult<Repository>;

    /// Looks up a repository by stable backend identifier.
    async fn get_repository(&self, id: &RepositoryId) -> ForgeResult<Option<Repository>>;

    /// Looks up a repository by human-facing owner/name path.
    async fn get_repository_by_path(
        &self,
        path: &RepositoryPath,
    ) -> ForgeResult<Option<Repository>>;

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

    /// Looks up an issue by its repository-scoped human-facing number.
    async fn get_issue_by_number(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
    ) -> ForgeResult<Option<Issue>>;

    /// Updates an issue.
    ///
    /// When `input.expected_version` is `Some`, the update is a compare-and-swap:
    /// it applies only if the stored [`Issue::version`](crate::Issue::version)
    /// equals the supplied token, and otherwise returns
    /// [`ForgeError::Conflict`] without mutating anything. When it is `None`, the
    /// update is unconditional. Either way, a successful update advances the
    /// stored version.
    async fn update_issue(&self, id: &IssueId, input: UpdateIssue) -> ForgeResult<Issue>;

    /// Adds a dependency link from an issue to another repository item number.
    async fn add_issue_dependency(&self, id: &IssueId, target: ItemNumber) -> ForgeResult<Issue>;

    /// Removes a dependency link from an issue to another repository item number.
    async fn remove_issue_dependency(&self, id: &IssueId, target: ItemNumber)
        -> ForgeResult<Issue>;

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

    /// Looks up a pull request by its repository-scoped human-facing number.
    async fn get_pull_request_by_number(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
    ) -> ForgeResult<Option<PullRequest>>;

    /// Updates a pull request.
    ///
    /// When `input.expected_version` is `Some`, the update is a compare-and-swap:
    /// it applies only if the stored
    /// [`PullRequest::version`](crate::PullRequest::version) equals the supplied
    /// token, and otherwise returns [`ForgeError::Conflict`] without mutating
    /// anything. When it is `None`, the update is unconditional. Either way, a
    /// successful update advances the stored version.
    async fn update_pull_request(
        &self,
        id: &PullRequestId,
        input: UpdatePullRequest,
    ) -> ForgeResult<PullRequest>;

    /// Adds a dependency link from a pull request to another repository item number.
    async fn add_pull_request_dependency(
        &self,
        id: &PullRequestId,
        target: ItemNumber,
    ) -> ForgeResult<PullRequest>;

    /// Removes a dependency link from a pull request to another repository item number.
    async fn remove_pull_request_dependency(
        &self,
        id: &PullRequestId,
        target: ItemNumber,
    ) -> ForgeResult<PullRequest>;

    /// Requests reviews from users on a pull request.
    async fn request_pull_request_reviewers(
        &self,
        id: &PullRequestId,
        input: RequestReviewers,
    ) -> ForgeResult<PullRequest>;

    /// Lists native review events on a pull request.
    async fn list_pull_request_reviews(
        &self,
        id: &PullRequestId,
    ) -> ForgeResult<Vec<PullRequestReview>>;

    /// Submits a native review event as the backend client's current user.
    async fn submit_pull_request_review(
        &self,
        id: &PullRequestId,
        input: CreatePullRequestReview,
    ) -> ForgeResult<PullRequestReview>;

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
