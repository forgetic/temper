use crate::ids::{
    CiJobId, CommentId, IssueId, ItemNumber, LabelId, PullRequestId, RepositoryId, ReviewId, UserId,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// Optimistic-concurrency version token for an issue or pull request.
///
/// Backends increment an artifact's version on every successful mutation of its
/// record. A caller that captures the version at read time can pass it back as
/// an [`UpdateIssue::expected_version`] / [`UpdatePullRequest::expected_version`]
/// precondition: the conditional update applies only if the stored version
/// still matches, and otherwise fails with
/// [`ForgeError::Conflict`](crate::ForgeError::Conflict). This is the portable
/// optimistic-concurrency primitive (see ADR 0013); a real forge maps it onto an
/// `ETag`/`If-Match` pair or an equivalent conditional write.
///
/// The token is a dedicated monotonic counter, not a timestamp. Reusing
/// `updated_at` would collide whenever two mutations share a clock value (the
/// reference backends advance the clock by a whole second per write), which
/// would silently defeat the precondition. A counter advances on every write, so
/// no two successive versions ever coincide.
#[derive(Copy, Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct Version(u64);

impl Version {
    /// The version assigned to a freshly created artifact.
    pub const INITIAL: Version = Version(1);

    /// Creates a version token from a raw counter value.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the raw counter value.
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Returns the next version, saturating at the maximum representable value.
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

impl Default for Version {
    /// The default token is [`Version::INITIAL`], so a record deserialized from a
    /// pre-versioning store reads as the initial version rather than failing.
    fn default() -> Self {
        Version::INITIAL
    }
}

impl fmt::Display for Version {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

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
    /// Repository item numbers this issue depends on.
    #[serde(default)]
    pub dependencies: Vec<ItemNumber>,
    /// Optimistic-concurrency token, advanced on every successful mutation.
    #[serde(default)]
    pub version: Version,
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
    /// Optimistic-concurrency precondition. When `Some`, the update applies only
    /// if the stored [`Issue::version`] equals this token, and otherwise fails
    /// with [`ForgeError::Conflict`](crate::ForgeError::Conflict). When `None`,
    /// the update is unconditional (the backward-compatible default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<Version>,
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
    /// Users whose review has been requested.
    #[serde(default)]
    pub requested_reviewers: Vec<UserId>,
    /// Repository item numbers this pull request depends on.
    #[serde(default)]
    pub dependencies: Vec<ItemNumber>,
    pub merge: Option<MergeRecord>,
    /// Optimistic-concurrency token, advanced on every successful mutation.
    #[serde(default)]
    pub version: Version,
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

/// Input used to request reviews from users on a pull request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RequestReviewers {
    pub reviewers: Vec<UserId>,
}

/// Portable pull-request review decision.
#[derive(Copy, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecision {
    Approved,
    ChangesRequested,
    Commented,
    Pending,
}

/// Pull-request review event recorded by a reviewer.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PullRequestReview {
    pub id: ReviewId,
    pub pull_request_id: PullRequestId,
    pub reviewer_id: UserId,
    pub decision: ReviewDecision,
    pub body: Option<String>,
    pub submitted_at: DateTime<Utc>,
}

/// Input used to submit a pull-request review as the backend's current user.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CreatePullRequestReview {
    pub decision: ReviewDecision,
    pub body: Option<String>,
}

/// Latest portable review decision for one reviewer.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReviewerDecision {
    pub reviewer_id: UserId,
    pub decision: ReviewDecision,
    pub submitted_at: DateTime<Utc>,
}

/// Portable aggregate review status for a pull request.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PullRequestReviewStatus {
    pub requested_reviewers: Vec<UserId>,
    pub latest_decisions: Vec<ReviewerDecision>,
    approved: bool,
    changes_requested: bool,
}

impl PullRequestReviewStatus {
    /// Computes the portable review aggregate from requested reviewers and
    /// review events.
    ///
    /// The latest non-comment review per reviewer wins. `Commented` reviews are
    /// ignored for the latest-decision map; `Pending` reviews count as a latest
    /// non-comment decision that blocks approval without requesting changes.
    /// The aggregate is approved only when at least one reviewer is requested,
    /// every requested reviewer's latest decision is `Approved`, and no
    /// reviewer's latest decision is `ChangesRequested`.
    pub fn from_reviews(requested_reviewers: &[UserId], reviews: &[PullRequestReview]) -> Self {
        let mut requested_reviewers = requested_reviewers.to_vec();
        requested_reviewers.sort();
        requested_reviewers.dedup();

        let mut latest: BTreeMap<UserId, &PullRequestReview> = BTreeMap::new();
        for review in reviews {
            if review.decision == ReviewDecision::Commented {
                continue;
            }
            latest
                .entry(review.reviewer_id.clone())
                .and_modify(|current| {
                    if review_is_newer(review, current) {
                        *current = review;
                    }
                })
                .or_insert(review);
        }

        let latest_decisions: Vec<ReviewerDecision> = latest
            .values()
            .map(|review| ReviewerDecision {
                reviewer_id: review.reviewer_id.clone(),
                decision: review.decision,
                submitted_at: review.submitted_at,
            })
            .collect();
        let changes_requested = latest_decisions
            .iter()
            .any(|decision| decision.decision == ReviewDecision::ChangesRequested);
        let approved = !requested_reviewers.is_empty()
            && !changes_requested
            && requested_reviewers.iter().all(|reviewer| {
                latest
                    .get(reviewer)
                    .is_some_and(|review| review.decision == ReviewDecision::Approved)
            });

        Self {
            requested_reviewers,
            latest_decisions,
            approved,
            changes_requested,
        }
    }

    /// Returns whether the portable aggregate is approved.
    pub fn is_approved(&self) -> bool {
        self.approved
    }

    /// Returns whether any latest reviewer decision requests changes.
    pub fn has_changes_requested(&self) -> bool {
        self.changes_requested
    }
}

fn review_is_newer(candidate: &PullRequestReview, current: &PullRequestReview) -> bool {
    candidate.submitted_at > current.submitted_at
        || (candidate.submitted_at == current.submitted_at && candidate.id >= current.id)
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
    /// Optimistic-concurrency precondition. When `Some`, the update applies only
    /// if the stored [`PullRequest::version`] equals this token, and otherwise
    /// fails with [`ForgeError::Conflict`](crate::ForgeError::Conflict). When
    /// `None`, the update is unconditional (the backward-compatible default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<Version>,
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
