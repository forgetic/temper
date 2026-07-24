use crate::ids::{CiJobId, IssueId, ItemNumber, PullRequestId, RepositoryId, UserId};
use crate::model::{
    CiJob, CiJobListing, CiJobStatus, CiRetryOutcome, CiRetryRequest, Comment, CreateComment,
    CreateIssue, CreatePullRequest, CreatePullRequestReview, CreateRepository, Issue, IssueState,
    Label, MergePullRequest, MergeRecord, PullRequest, PullRequestReview, PullRequestState,
    Repository, RepositoryPath, RequestReviewers, UpdateIssue, UpdatePullRequest, UpsertLabel,
    User,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

mod candidate;
pub use candidate::{
    CandidateLabelSelection, CandidateLabels, CandidateLifecycle, CandidateLifecycleBucket,
    IssueCandidateQuery, PullRequestCandidateQuery,
};

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

/// Whether issue and pull-request numbers can collide within a repository.
///
/// The conservative default is [`Self::Independent`]. A backend may advertise
/// [`Self::Shared`] only when one repository-scoped item number identifies at
/// most one issue or pull request.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ItemNumberNamespace {
    /// Issue and pull-request counters are independent and may collide.
    Independent,
    /// Issues and pull requests share one collision-free number sequence.
    Shared,
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
/// Defaults preserve the full Forge contract. Callers that only need scan
/// summary fields such as labels, body, state, and assignees may request
/// [`Self::summary`] to let backends skip expensive enrichment and detail
/// rendering. Pull-request branch refs, head/base SHAs, requested reviewers, and
/// merge records may be absent or empty in summary results; use exact gets or
/// full-detail lists when those fields matter.
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
///
/// All populated filters compose conjunctively. In particular, `labels`
/// requires every listed label; use [`IssueCandidateQuery`] for portable
/// any-label discovery.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct IssueQuery {
    pub state: Option<IssueState>,
    pub labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_contains: Option<String>,
    pub author_id: Option<UserId>,
    pub assignee_id: Option<UserId>,
    pub sort: Option<ItemSort>,
    /// Maximum number of results, applied after filtering and deterministic sorting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(default)]
    pub details: ItemListDetails,
}

/// Pull-request listing query.
///
/// All populated filters compose conjunctively. In particular, `labels`
/// requires every listed label; use [`PullRequestCandidateQuery`] for portable
/// any-label discovery.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PullRequestQuery {
    pub state: Option<PullRequestState>,
    pub labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_contains: Option<String>,
    pub author_id: Option<UserId>,
    pub assignee_id: Option<UserId>,
    pub sort: Option<ItemSort>,
    /// Maximum number of results, applied after filtering and deterministic sorting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
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
///
/// Every populated filter is conjunctive. In particular, setting both
/// `pull_request_id` and `commit_sha` returns only jobs that satisfy both
/// constraints; a backend must not treat either filter as an alternative.
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
    /// Describes whether issue and pull-request [`ItemNumber`] values can
    /// collide within one repository.
    ///
    /// This is a static backend capability and must not perform I/O. The
    /// independent default preserves issue-first resolution correctness for
    /// compatibility backends with separate counters.
    fn item_number_namespace(&self) -> ItemNumberNamespace {
        ItemNumberNamespace::Independent
    }

    /// Returns the cumulative provider HTTP request count when the backend can
    /// expose it without performing I/O. Callers use deltas only for debug
    /// measurements; correctness must never depend on this optional counter.
    fn provider_request_count(&self) -> Option<u64> {
        None
    }

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

    /// Lists issues in a repository. Query labels are conjunctive (all-of).
    async fn list_issues(
        &self,
        repo_id: &RepositoryId,
        query: IssueQuery,
    ) -> ForgeResult<Vec<Issue>>;

    /// Lists a consolidated lifecycle bucket of issue candidates.
    ///
    /// The compatibility fallback normalizes and deduplicates `AnyOf` labels,
    /// performs one existing conjunctive list call per label/state, unions by
    /// stable issue identity, and returns deterministic number/ID order.
    async fn list_issue_candidates(
        &self,
        repo_id: &RepositoryId,
        query: IssueCandidateQuery,
    ) -> ForgeResult<Vec<Issue>> {
        let labels = query.labels.normalized()?;
        let state = match query.lifecycle {
            CandidateLifecycle::Open => IssueState::Open,
            CandidateLifecycle::Terminal => IssueState::Closed,
        };
        let label_queries = labels
            .map(|labels| labels.into_iter().map(|label| vec![label]).collect())
            .unwrap_or_else(|| vec![Vec::new()]);
        let mut by_id = std::collections::BTreeMap::new();
        for labels in label_queries {
            for issue in self
                .list_issues(
                    repo_id,
                    IssueQuery {
                        state: Some(state),
                        labels,
                        details: query.details,
                        ..IssueQuery::default()
                    },
                )
                .await?
            {
                by_id.entry(issue.id.clone()).or_insert(issue);
            }
        }
        let mut issues = by_id.into_values().collect::<Vec<_>>();
        issues.sort_by(|left, right| {
            left.number
                .cmp(&right.number)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(issues)
    }

    /// Creates an issue in a repository.
    async fn create_issue(&self, repo_id: &RepositoryId, input: CreateIssue) -> ForgeResult<Issue>;

    /// Looks up an issue by stable backend identifier.
    async fn get_issue(&self, id: &IssueId) -> ForgeResult<Option<Issue>>;

    /// Looks up an issue by stable backend identifier with an explicit detail
    /// budget.
    ///
    /// The default preserves compatibility with existing backends by using the
    /// full exact read and stripping dependency data from summary results.
    /// Native backends should avoid dependency enrichment when
    /// `details.dependencies` is false.
    async fn get_issue_with_details(
        &self,
        id: &IssueId,
        details: ItemListDetails,
    ) -> ForgeResult<Option<Issue>> {
        let mut issue = self.get_issue(id).await?;
        if !details.dependencies {
            if let Some(issue) = &mut issue {
                issue.dependencies.clear();
            }
        }
        Ok(issue)
    }

    /// Looks up an issue by its repository-scoped human-facing number.
    async fn get_issue_by_number(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
    ) -> ForgeResult<Option<Issue>>;

    /// Looks up an issue by number with an explicit detail budget.
    ///
    /// The compatibility default delegates to the full exact read. Backends
    /// with expensive native dependency APIs should override this method.
    async fn get_issue_by_number_with_details(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
        details: ItemListDetails,
    ) -> ForgeResult<Option<Issue>> {
        let mut issue = self.get_issue_by_number(repo_id, number).await?;
        if !details.dependencies {
            if let Some(issue) = &mut issue {
                issue.dependencies.clear();
            }
        }
        Ok(issue)
    }

    /// Updates an issue.
    ///
    /// When `input.expected_version` is `Some`, the update is a compare-and-swap:
    /// it applies only if the stored [`Issue::version`](crate::Issue::version)
    /// equals the supplied token, and otherwise returns
    /// [`ForgeError::Conflict`] without mutating anything. When it is `None`, the
    /// update is unconditional. Either way, a successful update advances the
    /// stored version.
    async fn update_issue(&self, id: &IssueId, input: UpdateIssue) -> ForgeResult<Issue>;

    /// Updates an issue using a validated current representation.
    ///
    /// Carrying the snapshot lets hosted backends derive label and assignee
    /// replacements without an unconditional read-before-write and return the
    /// mutation response without a post-write exact read. The compatibility
    /// default delegates to [`Self::update_issue`].
    async fn update_issue_from_snapshot(
        &self,
        current: &Issue,
        input: UpdateIssue,
    ) -> ForgeResult<Issue> {
        if input
            .expected_version
            .is_some_and(|expected| expected != current.version)
        {
            return Err(ForgeError::Conflict(format!(
                "stale conditional update of issue {}: snapshot version is {}",
                current.id, current.version
            )));
        }
        self.update_issue(&current.id, input).await
    }

    /// Adds a dependency link from an issue to another repository item number.
    async fn add_issue_dependency(&self, id: &IssueId, target: ItemNumber) -> ForgeResult<Issue>;

    /// Removes a dependency link from an issue to another repository item number.
    async fn remove_issue_dependency(&self, id: &IssueId, target: ItemNumber)
    -> ForgeResult<Issue>;

    /// Lists comments on an issue.
    async fn list_issue_comments(&self, id: &IssueId) -> ForgeResult<Vec<Comment>>;

    /// Adds a comment to an issue.
    async fn add_issue_comment(&self, id: &IssueId, input: CreateComment) -> ForgeResult<Comment>;

    /// Lists pull requests in a repository. Query labels are conjunctive (all-of).
    async fn list_pull_requests(
        &self,
        repo_id: &RepositoryId,
        query: PullRequestQuery,
    ) -> ForgeResult<Vec<PullRequest>>;

    /// Lists a consolidated lifecycle bucket of pull-request candidates.
    ///
    /// The deterministic compatibility fallback follows the issue fallback and
    /// reads both closed and merged states for a terminal bucket.
    async fn list_pull_request_candidates(
        &self,
        repo_id: &RepositoryId,
        query: PullRequestCandidateQuery,
    ) -> ForgeResult<Vec<PullRequest>> {
        let labels = query.labels.normalized()?;
        let states: &[PullRequestState] = match query.lifecycle {
            CandidateLifecycle::Open => &[PullRequestState::Open],
            CandidateLifecycle::Terminal => &[PullRequestState::Closed, PullRequestState::Merged],
        };
        let label_queries: Vec<Vec<String>> = labels
            .map(|labels| labels.into_iter().map(|label| vec![label]).collect())
            .unwrap_or_else(|| vec![Vec::new()]);
        let mut by_id = std::collections::BTreeMap::new();
        for state in states {
            for labels in &label_queries {
                for pull_request in self
                    .list_pull_requests(
                        repo_id,
                        PullRequestQuery {
                            state: Some(*state),
                            labels: labels.clone(),
                            details: query.details,
                            ..PullRequestQuery::default()
                        },
                    )
                    .await?
                {
                    by_id.entry(pull_request.id.clone()).or_insert(pull_request);
                }
            }
        }
        let mut pull_requests = by_id.into_values().collect::<Vec<_>>();
        pull_requests.sort_by(|left, right| {
            left.number
                .cmp(&right.number)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(pull_requests)
    }

    /// Creates a pull request in a repository.
    async fn create_pull_request(
        &self,
        repo_id: &RepositoryId,
        input: CreatePullRequest,
    ) -> ForgeResult<PullRequest>;

    /// Looks up a pull request by stable backend identifier.
    async fn get_pull_request(&self, id: &PullRequestId) -> ForgeResult<Option<PullRequest>>;

    /// Looks up a pull request by stable identifier with an explicit detail budget.
    ///
    /// The compatibility default performs the historical full read and clears
    /// dependencies for a summary result.
    async fn get_pull_request_with_details(
        &self,
        id: &PullRequestId,
        details: ItemListDetails,
    ) -> ForgeResult<Option<PullRequest>> {
        let mut pull_request = self.get_pull_request(id).await?;
        if !details.dependencies {
            if let Some(pull_request) = &mut pull_request {
                pull_request.dependencies.clear();
            }
        }
        Ok(pull_request)
    }

    /// Looks up a pull request by its repository-scoped human-facing number.
    async fn get_pull_request_by_number(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
    ) -> ForgeResult<Option<PullRequest>>;

    /// Looks up a pull request by number with an explicit detail budget.
    ///
    /// The compatibility default performs the historical full read and clears
    /// dependencies for a summary result.
    async fn get_pull_request_by_number_with_details(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
        details: ItemListDetails,
    ) -> ForgeResult<Option<PullRequest>> {
        let mut pull_request = self.get_pull_request_by_number(repo_id, number).await?;
        if !details.dependencies {
            if let Some(pull_request) = &mut pull_request {
                pull_request.dependencies.clear();
            }
        }
        Ok(pull_request)
    }

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

    /// Merges a pull request. Backends that support merge-time source branch
    /// cleanup honor [`MergePullRequest::delete_source_branch`].
    async fn merge_pull_request(
        &self,
        id: &PullRequestId,
        input: MergePullRequest,
    ) -> ForgeResult<MergeRecord>;

    /// Lists CI jobs in a repository. Populated query filters compose
    /// conjunctively, including pull-request ID plus commit SHA.
    async fn list_ci_jobs(
        &self,
        repo_id: &RepositoryId,
        query: CiJobQuery,
    ) -> ForgeResult<Vec<CiJob>>;

    /// Lists CI jobs and separately reports matching provider CI presence.
    ///
    /// The presence bit uses the query's repository, pull-request, and commit
    /// ownership filters, but is independent of its job-status filter. It may be
    /// true with no returned jobs when a provider has registered a workflow run
    /// that is still waiting for a runner to materialize its jobs.
    async fn list_ci_jobs_with_presence(
        &self,
        repo_id: &RepositoryId,
        query: CiJobQuery,
    ) -> ForgeResult<CiJobListing>;

    /// Requests a provider retry of exactly one freshly observed CI attempt.
    ///
    /// The repository, pull request, current head, run, attempt, and latest job
    /// fingerprint are all mandatory fences. Implementations must revalidate
    /// them before mutation and fail closed with a typed outcome when retry is
    /// unavailable, stale, rejected, or uncertain. A retry must never mutate
    /// source control merely to create a new run.
    async fn retry_ci_attempt(&self, request: CiRetryRequest) -> ForgeResult<CiRetryOutcome>;

    /// Looks up a CI job by stable backend identifier.
    async fn get_ci_job(&self, id: &CiJobId) -> ForgeResult<Option<CiJob>>;
}
