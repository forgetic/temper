//! Forgejo provider DTOs (serde).
//!
//! This module holds the data-transfer objects the user/repository/label phase
//! (02) and the pull-request/comment/review phase (04) need. Later phases extend
//! it with issue, dependency, and Actions DTOs. The DTOs are deliberately
//! lenient: unknown fields are ignored and optional fields default, so the
//! backend tolerates provider version drift.

#![allow(dead_code)]

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
}
