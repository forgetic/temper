//! Forgejo provider DTOs (serde).
//!
//! This module holds the data-transfer objects the user/repository/label phase
//! (02) and the pull-request/comment/review phase (04) need. Later phases extend
//! it with issue, dependency, and Actions DTOs. The DTOs are deliberately
//! lenient: unknown fields are ignored and optional fields default, so the
//! backend tolerates provider version drift.
//!
//! The DTOs are grouped into submodules by domain ([`items`] for the shared
//! user/repo/label/comment building blocks, [`issues`] for issue rows,
//! [`pulls`] for pull requests and reviews, [`actions`] for CI runs/jobs) and
//! re-exported here so callers use a single `crate::types::*` surface.

mod actions;
mod issues;
mod pulls;

use chrono::{DateTime, Utc};
use serde::Deserialize;

pub(crate) use actions::{ActionJobDto, ActionRunDto};
pub(crate) use issues::{DependencyRefDto, IssueDto, PullRequestMarkerDto};
pub(crate) use pulls::{PrBranchDto, PrRepoDto, PullRequestDto, ReviewDto};

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
}
