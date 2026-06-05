//! Forgejo provider DTOs (serde).
//!
//! This module holds the data-transfer objects the user/repository/label phase
//! (02) and the pull-request/comment/review phase (04) need. Later phases extend
//! it with issue, dependency, and Actions DTOs. The DTOs are deliberately
//! lenient: unknown fields are ignored and optional fields default, so the
//! backend tolerates provider version drift.

use crate::ci_time::deserialize_flexible_opt_datetime;
use chrono::{DateTime, Utc};
use serde::Deserialize;

/// Forgejo user account.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct UserDto {
    pub login: String,
    #[serde(default)]
    pub full_name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
}

/// Forgejo label attached to a repository.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct LabelDto {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// Forgejo repository.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct RepositoryDto {
    pub owner: UserDto,
    pub name: String,
    #[serde(default)]
    pub full_name: Option<String>,
    #[serde(default)]
    pub default_branch: String,
    #[serde(default)]
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Forgejo issue or pull-request comment.
///
/// Pull-request comments are issue comments on Forgejo, so this DTO maps both.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct CommentDto {
    pub id: u64,
    pub user: UserDto,
    #[serde(default)]
    pub body: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Forgejo issue.
///
/// Forgejo serves both issues and pull requests through the issue endpoints, so
/// a pull request looked up by number also deserializes into this DTO. As with
/// pull requests, absent collections serialize as JSON `null`, so they are
/// optional and mapping treats `None` as an empty set.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct IssueDto {
    pub number: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
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
    /// Forgejo serves pull requests through the issue endpoints, tagging each
    /// PR-as-issue row with a `pull_request` object (issues serialize it as
    /// JSON `null`). The backend only needs the marker's presence to exclude
    /// pull requests from issue results, so the contents are ignored.
    #[serde(default)]
    pub pull_request: Option<PullRequestMarkerDto>,
}

/// Marker object Forgejo attaches to a PR-as-issue row's `pull_request` field.
///
/// Its presence (a non-null object) distinguishes a pull request from a genuine
/// issue. Forgejo 7.0.x also exposes merge-state hints here, which let labelled
/// summary scans separate portable `closed` from `merged` without rendering the
/// expensive pull-request detail endpoint.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
pub(crate) struct PullRequestMarkerDto {
    #[serde(default)]
    pub merged: Option<bool>,
    #[serde(default)]
    pub merged_at: Option<DateTime<Utc>>,
}

impl IssueDto {
    /// Reports whether this row is a pull request masquerading as an issue.
    pub(crate) fn is_pull_request(&self) -> bool {
        self.pull_request.is_some()
    }
}

/// Minimal reference to an issue or pull request returned by the dependencies
/// endpoint.
///
/// Forgejo's `GET /issues/{index}/dependencies` returns full issue objects, but
/// the backend only needs the repository-scoped number to build a portable
/// dependency list, so this DTO captures just that and ignores the rest.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct DependencyRefDto {
    pub number: u64,
}

/// Repository reference embedded in a pull-request branch (`head`/`base`).
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
pub(crate) struct PrRepoDto {
    #[serde(default)]
    pub full_name: Option<String>,
    #[serde(default)]
    pub owner: Option<UserDto>,
    #[serde(default)]
    pub name: Option<String>,
}

/// One side (`head` or `base`) of a Forgejo pull request.
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

/// Forgejo pull request.
///
/// `assignees`, `requested_reviewers`, and `labels` are modeled as optional
/// vectors because Forgejo serializes absent collections as JSON `null` rather
/// than an empty array; mapping treats `None` as an empty set.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct PullRequestDto {
    pub number: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub merged: bool,
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

/// Forgejo pull-request review event.
///
/// `state` is the provider review verdict string (`APPROVED`, `REQUEST_CHANGES`,
/// `COMMENT`, `PENDING`, or `REQUEST_REVIEW`). `dismissed`/`stale` mark reviews
/// the portable aggregate must not count.
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
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub dismissed: bool,
    #[serde(default)]
    pub stale: bool,
}

/// Forgejo Actions workflow run.
///
/// Returned by `GET /repos/{owner}/{repo}/actions/runs` (wrapped under a
/// `workflow_runs` array). Fields are lenient because run payloads vary across
/// Forgejo versions; timestamps may be RFC3339 strings (`*_at`) or unix-epoch
/// integers (`created`/`updated`), so they decode through the tolerant helper.
#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct ActionRunDto {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub index_in_repo: u64,
    #[serde(default)]
    pub run_number: u64,
    // Decoded for provider-shape fidelity; CI status is derived from per-task
    // attempts (see `crate::ci`), so the run-level status is not read directly.
    #[serde(default)]
    #[allow(dead_code)]
    pub status: String,
    #[serde(default)]
    pub event: String,
    #[serde(default)]
    pub prettyref: String,
    #[serde(default)]
    pub head_branch: String,
    #[serde(default)]
    pub head_sha: String,
    #[serde(default)]
    pub commit_sha: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub html_url: String,
    #[serde(default)]
    pub event_payload: String,
    #[serde(default, deserialize_with = "deserialize_flexible_opt_datetime")]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "deserialize_flexible_opt_datetime")]
    pub created: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "deserialize_flexible_opt_datetime")]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "deserialize_flexible_opt_datetime")]
    pub updated: Option<DateTime<Utc>>,
}

/// Forgejo Actions task (a single job within a run).
///
/// Returned by `GET /repos/{owner}/{repo}/actions/tasks` (also wrapped under a
/// `workflow_runs` array). `run_number` ties a task to its run's repo-stable
/// index. Timestamps use the same tolerant decoding as [`ActionRunDto`].
#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct ActionTaskDto {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub run_number: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub commit_sha: String,
    #[serde(default)]
    pub head_sha: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub html_url: String,
    #[serde(default, deserialize_with = "deserialize_flexible_opt_datetime")]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "deserialize_flexible_opt_datetime")]
    pub created: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "deserialize_flexible_opt_datetime")]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "deserialize_flexible_opt_datetime")]
    pub updated: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "deserialize_flexible_opt_datetime")]
    pub run_started_at: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "deserialize_flexible_opt_datetime")]
    pub started: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "deserialize_flexible_opt_datetime")]
    pub stopped: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_user_with_optional_fields() {
        let user: UserDto = serde_json::from_str(
            r#"{"login":"octocat","full_name":"The Octocat","email":"cat@example.com","extra":true}"#,
        )
        .unwrap();
        assert_eq!(user.login, "octocat");
        assert_eq!(user.full_name.as_deref(), Some("The Octocat"));
        assert_eq!(user.email.as_deref(), Some("cat@example.com"));
    }

    #[test]
    fn user_tolerates_missing_optional_fields() {
        let user: UserDto = serde_json::from_str(r#"{"login":"ghost"}"#).unwrap();
        assert_eq!(user.login, "ghost");
        assert_eq!(user.full_name, None);
        assert_eq!(user.email, None);
    }

    #[test]
    fn deserializes_label() {
        let label: LabelDto = serde_json::from_str(
            r#"{"id":7,"name":"ready","color":"00ff00","description":"ready to start"}"#,
        )
        .unwrap();
        assert_eq!(label.id, 7);
        assert_eq!(label.name, "ready");
        assert_eq!(label.color.as_deref(), Some("00ff00"));
        assert_eq!(label.description.as_deref(), Some("ready to start"));
    }

    #[test]
    fn deserializes_repository_with_owner_object() {
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
        assert_eq!(repo.full_name.as_deref(), Some("acme/widgets"));
        assert_eq!(repo.default_branch, "main");
        assert_eq!(repo.description.as_deref(), Some("widget factory"));
    }

    #[test]
    fn deserializes_comment() {
        let comment: CommentDto = serde_json::from_str(
            r#"{
                "id": 91,
                "user": {"login": "reviewer"},
                "body": "looks good",
                "created_at": "2024-03-01T00:00:00Z",
                "updated_at": "2024-03-01T00:05:00Z",
                "extra": 1
            }"#,
        )
        .unwrap();
        assert_eq!(comment.id, 91);
        assert_eq!(comment.user.login, "reviewer");
        assert_eq!(comment.body, "looks good");
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
                "pull_request": {"merged": false, "url": "http://example/pulls/8"}
            }"#,
        )
        .unwrap();
        assert!(pull_as_issue.is_pull_request());
    }

    #[test]
    fn deserializes_pull_request_with_null_collections() {
        let pull: PullRequestDto = serde_json::from_str(
            r#"{
                "number": 42,
                "title": "Add widget",
                "body": "details",
                "state": "open",
                "merged": false,
                "user": {"login": "author"},
                "head": {"label": "author:feature", "ref": "feature", "sha": "headsha"},
                "base": {"label": "main", "ref": "main", "sha": "basesha"},
                "labels": null,
                "assignees": null,
                "requested_reviewers": null,
                "created_at": "2024-03-01T00:00:00Z",
                "updated_at": "2024-03-02T00:00:00Z"
            }"#,
        )
        .unwrap();
        assert_eq!(pull.number, 42);
        assert!(!pull.merged);
        assert_eq!(pull.head.unwrap().sha.as_deref(), Some("headsha"));
        assert_eq!(pull.labels, None);
        assert_eq!(pull.assignees, None);
        assert_eq!(pull.requested_reviewers, None);
    }

    #[test]
    fn deserializes_merged_pull_request() {
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
                "labels": [{"id": 3, "name": "ready"}],
                "assignees": [{"login": "bob"}],
                "created_at": "2024-03-01T00:00:00Z",
                "updated_at": "2024-04-01T00:00:00Z",
                "closed_at": "2024-04-01T00:00:00Z"
            }"#,
        )
        .unwrap();
        assert!(pull.merged);
        assert_eq!(pull.merge_commit_sha.as_deref(), Some("mergesha"));
        assert_eq!(pull.merged_by.unwrap().login, "maintainer");
        assert_eq!(pull.labels.unwrap().len(), 1);
    }

    #[test]
    fn deserializes_review() {
        let review: ReviewDto = serde_json::from_str(
            r#"{
                "id": 12,
                "user": {"login": "carol"},
                "body": "approving",
                "state": "APPROVED",
                "submitted_at": "2024-03-03T00:00:00Z",
                "dismissed": false,
                "stale": false
            }"#,
        )
        .unwrap();
        assert_eq!(review.id, 12);
        assert_eq!(review.state, "APPROVED");
        assert!(!review.dismissed);
    }

    #[test]
    fn deserializes_action_run_with_string_and_integer_timestamps() {
        let run: ActionRunDto = serde_json::from_str(
            r##"{
                "index_in_repo": 10,
                "run_number": 10,
                "status": "success",
                "event": "pull_request",
                "prettyref": "#7",
                "head_branch": "feature",
                "head_sha": "abcdef1234567",
                "created_at": "2024-01-02T00:00:00Z",
                "updated": 1700000000,
                "extra": true
            }"##,
        )
        .unwrap();
        assert_eq!(run.index_in_repo, 10);
        assert_eq!(run.prettyref, "#7");
        assert!(run.created_at.is_some());
        assert!(run.updated.is_some());
        assert_eq!(run.created, None);
    }

    #[test]
    fn deserializes_action_task_with_defaults() {
        let task: ActionTaskDto = serde_json::from_str(
            r#"{
                "id": 3,
                "run_number": 10,
                "name": "build",
                "status": "success",
                "head_sha": "abcdef1234567",
                "created_at": "2024-01-02T03:04:05Z"
            }"#,
        )
        .unwrap();
        assert_eq!(task.id, 3);
        assert_eq!(task.run_number, 10);
        assert_eq!(task.name, "build");
        assert_eq!(task.commit_sha, "");
        assert!(task.created_at.is_some());
        assert_eq!(task.stopped, None);
    }
}
