//! GitHub provider DTOs (serde).
//!
//! The DTOs are deliberately lenient: unknown fields are ignored and optional
//! fields default, so the backend tolerates provider drift between
//! `api.github.com` and GitHub Enterprise versions. GitHub serializes unset
//! text fields (issue/pull bodies, comment bodies) as JSON `null`, so those are
//! optional and mapping treats `None` as empty.

use chrono::{DateTime, Utc};
use serde::Deserialize;

/// GitHub user account.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct UserDto {
    pub login: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
}

/// GitHub label attached to a repository.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct LabelDto {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// GitHub repository.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct RepositoryDto {
    pub owner: UserDto,
    pub name: String,
    #[serde(default)]
    pub default_branch: String,
    #[serde(default)]
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// GitHub issue or pull-request comment.
///
/// Pull-request (conversation) comments are issue comments on GitHub, so this
/// DTO maps both.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct CommentDto {
    pub id: u64,
    pub user: UserDto,
    #[serde(default)]
    pub body: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// GitHub issue.
///
/// GitHub serves both issues and pull requests through the issue endpoints, so
/// a pull request looked up by number also deserializes into this DTO; the
/// `pull_request` marker distinguishes the two.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct IssueDto {
    pub number: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub state: String,
    pub user: UserDto,
    #[serde(default)]
    pub labels: Option<Vec<LabelDto>>,
    #[serde(default)]
    pub assignees: Option<Vec<UserDto>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub closed_at: Option<DateTime<Utc>>,
    /// Present (a non-null object) when this row is actually a pull request.
    ///
    /// GitHub serves pull requests through the issue endpoints, tagging each
    /// PR-as-issue row with a `pull_request` object carrying the PR URLs and a
    /// `merged_at` timestamp. The backend only needs the marker's presence to
    /// exclude pull requests from issue results.
    #[serde(default)]
    pub pull_request: Option<PullRequestMarkerDto>,
}

/// Marker object GitHub attaches to a PR-as-issue row's `pull_request` field.
///
/// Its presence (a non-null object) distinguishes a pull request from a
/// genuine issue; `merged_at` additionally distinguishes merged from closed.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
pub(crate) struct PullRequestMarkerDto {
    #[serde(default)]
    pub merged_at: Option<DateTime<Utc>>,
}

impl IssueDto {
    /// Reports whether this row is a pull request masquerading as an issue.
    pub(crate) fn is_pull_request(&self) -> bool {
        self.pull_request.is_some()
    }
}

/// Repository reference embedded in a pull-request branch (`head`/`base`).
///
/// GitHub serializes the repository of a deleted fork as JSON `null`, so every
/// field is optional.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
pub(crate) struct PrRepoDto {
    #[serde(default)]
    pub full_name: Option<String>,
    #[serde(default)]
    pub owner: Option<UserDto>,
    #[serde(default)]
    pub name: Option<String>,
}

/// One side (`head` or `base`) of a GitHub pull request.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
pub(crate) struct PrBranchDto {
    #[serde(default)]
    pub label: Option<String>,
    #[serde(rename = "ref", default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub sha: Option<String>,
    #[serde(default)]
    pub repo: Option<PrRepoDto>,
}

/// GitHub pull request.
///
/// The list endpoint (`GET /pulls`) omits the `merged` boolean (only the detail
/// endpoint includes it), so merged state falls back to `merged_at` presence.
/// Absent collections are modeled as optional vectors for lenience; mapping
/// treats `None` as an empty set.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct PullRequestDto {
    pub number: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub merged: Option<bool>,
    #[serde(default)]
    pub merged_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub merge_commit_sha: Option<String>,
    #[serde(default)]
    pub merged_by: Option<UserDto>,
    pub user: UserDto,
    #[serde(default)]
    pub head: Option<PrBranchDto>,
    #[serde(default)]
    pub base: Option<PrBranchDto>,
    #[serde(default)]
    pub labels: Option<Vec<LabelDto>>,
    #[serde(default)]
    pub assignees: Option<Vec<UserDto>>,
    #[serde(default)]
    pub requested_reviewers: Option<Vec<UserDto>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub closed_at: Option<DateTime<Utc>>,
}

/// GitHub pull-request review event.
///
/// `state` is the provider review verdict string (`APPROVED`,
/// `CHANGES_REQUESTED`, `COMMENTED`, `PENDING`, or `DISMISSED`).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct ReviewDto {
    pub id: u64,
    pub user: UserDto,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub submitted_at: Option<DateTime<Utc>>,
}

/// Result body of a successful pull-request merge (`PUT /pulls/{n}/merge`).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct MergeResultDto {
    #[serde(default)]
    pub sha: Option<String>,
    #[serde(default)]
    pub merged: bool,
    #[serde(default)]
    pub message: Option<String>,
}

/// GitHub Actions workflow run (one element of the `workflow_runs` envelope).
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
pub(crate) struct WorkflowRunDto {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub head_sha: String,
}

/// Envelope wrapping `GET /repos/{owner}/{repo}/actions/runs`.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
pub(crate) struct WorkflowRunsEnvelopeDto {
    #[serde(default)]
    pub workflow_runs: Vec<WorkflowRunDto>,
}

/// GitHub Actions workflow job.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
pub(crate) struct WorkflowJobDto {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub head_sha: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub conclusion: Option<String>,
    #[serde(default)]
    pub html_url: Option<String>,
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub completed_at: Option<DateTime<Utc>>,
}

/// Envelope wrapping `GET /repos/{owner}/{repo}/actions/runs/{id}/jobs`.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
pub(crate) struct WorkflowJobsEnvelopeDto {
    #[serde(default)]
    pub jobs: Vec<WorkflowJobDto>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_user_with_optional_fields() {
        let user: UserDto = serde_json::from_str(
            r#"{"login":"octocat","name":"The Octocat","email":"cat@example.com","extra":true}"#,
        )
        .unwrap();
        assert_eq!(user.login, "octocat");
        assert_eq!(user.name.as_deref(), Some("The Octocat"));
        assert_eq!(user.email.as_deref(), Some("cat@example.com"));
    }

    #[test]
    fn user_tolerates_missing_and_null_optional_fields() {
        let user: UserDto = serde_json::from_str(r#"{"login":"ghost","name":null}"#).unwrap();
        assert_eq!(user.login, "ghost");
        assert_eq!(user.name, None);
        assert_eq!(user.email, None);
    }

    #[test]
    fn deserializes_repository() {
        let repo: RepositoryDto = serde_json::from_str(
            r#"{
                "owner": {"login": "acme"},
                "name": "widgets",
                "full_name": "acme/widgets",
                "default_branch": "main",
                "description": "widget factory",
                "created_at": "2024-01-01T00:00:00Z",
                "updated_at": "2024-02-02T12:00:00Z"
            }"#,
        )
        .unwrap();
        assert_eq!(repo.owner.login, "acme");
        assert_eq!(repo.name, "widgets");
        assert_eq!(repo.default_branch, "main");
        assert_eq!(repo.description.as_deref(), Some("widget factory"));
    }

    #[test]
    fn deserializes_comment_with_null_body() {
        let comment: CommentDto = serde_json::from_str(
            r#"{
                "id": 91,
                "user": {"login": "reviewer"},
                "body": null,
                "created_at": "2024-03-01T00:00:00Z",
                "updated_at": "2024-03-01T00:05:00Z"
            }"#,
        )
        .unwrap();
        assert_eq!(comment.id, 91);
        assert_eq!(comment.body, None);
    }

    #[test]
    fn issue_pull_request_marker_distinguishes_rows() {
        let issue: IssueDto = serde_json::from_str(
            r#"{
                "number": 7,
                "user": {"login": "author"},
                "created_at": "2024-03-01T00:00:00Z",
                "updated_at": "2024-03-02T00:00:00Z",
                "pull_request": null
            }"#,
        )
        .unwrap();
        assert!(!issue.is_pull_request());

        let pull_as_issue: IssueDto = serde_json::from_str(
            r#"{
                "number": 8,
                "user": {"login": "author"},
                "created_at": "2024-03-01T00:00:00Z",
                "updated_at": "2024-03-02T00:00:00Z",
                "pull_request": {"url": "https://api.github.com/repos/acme/widgets/pulls/8", "merged_at": null}
            }"#,
        )
        .unwrap();
        assert!(pull_as_issue.is_pull_request());
    }

    #[test]
    fn deserializes_pull_request_without_merged_flag() {
        // The `/pulls` list endpoint omits `merged`; only `merged_at` signals it.
        let pull: PullRequestDto = serde_json::from_str(
            r#"{
                "number": 42,
                "title": "Add widget",
                "body": null,
                "state": "open",
                "user": {"login": "author"},
                "head": {"label": "author:feature", "ref": "feature", "sha": "headsha", "repo": null},
                "base": {"label": "acme:main", "ref": "main", "sha": "basesha"},
                "labels": [],
                "created_at": "2024-03-01T00:00:00Z",
                "updated_at": "2024-03-02T00:00:00Z"
            }"#,
        )
        .unwrap();
        assert_eq!(pull.number, 42);
        assert_eq!(pull.merged, None);
        assert_eq!(pull.body, None);
        assert_eq!(pull.head.unwrap().sha.as_deref(), Some("headsha"));
    }

    #[test]
    fn deserializes_merged_pull_request_detail() {
        let pull: PullRequestDto = serde_json::from_str(
            r#"{
                "number": 7,
                "title": "merged",
                "state": "closed",
                "merged": true,
                "merged_at": "2024-04-01T00:00:00Z",
                "merge_commit_sha": "mergesha",
                "merged_by": {"login": "maintainer"},
                "user": {"login": "author"},
                "created_at": "2024-03-01T00:00:00Z",
                "updated_at": "2024-04-01T00:00:00Z",
                "closed_at": "2024-04-01T00:00:00Z"
            }"#,
        )
        .unwrap();
        assert_eq!(pull.merged, Some(true));
        assert_eq!(pull.merge_commit_sha.as_deref(), Some("mergesha"));
        assert_eq!(pull.merged_by.unwrap().login, "maintainer");
    }

    #[test]
    fn deserializes_review() {
        let review: ReviewDto = serde_json::from_str(
            r#"{
                "id": 12,
                "user": {"login": "carol"},
                "body": "approving",
                "state": "APPROVED",
                "submitted_at": "2024-03-03T00:00:00Z"
            }"#,
        )
        .unwrap();
        assert_eq!(review.id, 12);
        assert_eq!(review.state, "APPROVED");
    }

    #[test]
    fn deserializes_actions_envelopes() {
        let runs: WorkflowRunsEnvelopeDto = serde_json::from_str(
            r#"{
                "total_count": 1,
                "workflow_runs": [{"id": 30433642, "head_sha": "abcdef1", "status": "completed"}]
            }"#,
        )
        .unwrap();
        assert_eq!(runs.workflow_runs.len(), 1);
        assert_eq!(runs.workflow_runs[0].id, 30433642);

        let jobs: WorkflowJobsEnvelopeDto = serde_json::from_str(
            r#"{
                "total_count": 1,
                "jobs": [{
                    "id": 399444496,
                    "run_id": 29679449,
                    "head_sha": "abcdef1",
                    "name": "build",
                    "status": "completed",
                    "conclusion": "success",
                    "html_url": "https://github.com/acme/widgets/runs/399444496",
                    "created_at": "2024-01-02T03:00:00Z",
                    "started_at": "2024-01-02T03:04:05Z",
                    "completed_at": "2024-01-02T03:10:00Z"
                }]
            }"#,
        )
        .unwrap();
        assert_eq!(jobs.jobs.len(), 1);
        assert_eq!(jobs.jobs[0].conclusion.as_deref(), Some("success"));
    }

    #[test]
    fn deserializes_merge_result() {
        let merge: MergeResultDto = serde_json::from_str(
            r#"{"sha": "mergesha", "merged": true, "message": "Pull Request successfully merged"}"#,
        )
        .unwrap();
        assert!(merge.merged);
        assert_eq!(merge.sha.as_deref(), Some("mergesha"));
    }
}
