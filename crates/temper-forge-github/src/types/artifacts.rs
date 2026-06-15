//! DTOs for the simple artifacts: users, labels, repositories, and comments.

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
}
