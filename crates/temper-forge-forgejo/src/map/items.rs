//! Conversions for the shared item DTOs: users, repositories, labels, comments,
//! and issues.

use super::{map_logins, non_empty, normalize, sorted_dedup, sorted_dedup_users};
use crate::ids::{
    RepoCoord, format_comment_id, format_issue_id, format_label_id, format_repository_id,
    format_user_id,
};
use crate::types::{CommentDto, IssueDto, LabelDto, RepositoryDto, UserDto};
use temper_forge::{Comment, Issue, IssueState, ItemNumber, Label, Repository, User, Version};

/// Maps a Forgejo user DTO into a portable [`User`].
///
/// The Forgejo login is both the portable [`UserId`](temper_forge::UserId) and
/// the human-facing handle. Empty `full_name`/`email` strings (Forgejo's
/// "unset" form) map to `None`, matching the reference backends' optional
/// fields.
pub(crate) fn map_user(dto: UserDto) -> User {
    User {
        id: format_user_id(&dto.login),
        handle: dto.login,
        display_name: non_empty(dto.full_name),
        email: non_empty(dto.email),
    }
}

/// Maps a Forgejo repository DTO into a portable [`Repository`].
pub(crate) fn map_repository(dto: RepositoryDto) -> Repository {
    let repo = RepoCoord::new(dto.owner.login, dto.name);
    Repository {
        id: format_repository_id(&repo),
        owner: repo.owner,
        name: repo.name,
        default_branch: dto.default_branch,
        description: non_empty(dto.description),
        created_at: dto.created_at,
        updated_at: dto.updated_at,
    }
}

/// Maps a Forgejo label DTO into a portable [`Label`] scoped to `repo`.
///
/// The numeric provider id becomes the prefixed opaque
/// [`LabelId`](temper_forge::LabelId); empty color/description strings map to
/// `None`.
pub(crate) fn map_label(repo: &RepoCoord, dto: LabelDto) -> Label {
    Label {
        id: format_label_id(repo, dto.id),
        repo_id: format_repository_id(repo),
        name: dto.name,
        color: non_empty(dto.color),
        description: non_empty(dto.description),
    }
}

/// Maps a Forgejo comment DTO into a portable [`Comment`].
pub(crate) fn map_comment(repo: &RepoCoord, dto: CommentDto) -> Comment {
    Comment {
        id: format_comment_id(repo, dto.id),
        author_id: format_user_id(&dto.user.login),
        body: dto.body,
        created_at: dto.created_at,
        updated_at: dto.updated_at,
    }
}

/// Maps a Forgejo issue DTO into a portable [`Issue`].
///
/// `version` is set to [`Version::INITIAL`]; callers that have the response
/// validator overwrite it through the backend's version cache.
/// `dependencies` is left empty here and populated by the dependency-link
/// enrichment step (see [`crate::dependencies`]).
pub(crate) fn map_issue(repo: &RepoCoord, dto: IssueDto) -> Issue {
    let number = ItemNumber::new(dto.number);
    let state = if normalize(&dto.state) == "closed" {
        IssueState::Closed
    } else {
        IssueState::Open
    };
    let labels = sorted_dedup(
        dto.labels
            .unwrap_or_default()
            .into_iter()
            .map(|label| label.name)
            .collect(),
    );
    let assignees = sorted_dedup_users(map_logins(dto.assignees));

    Issue {
        id: format_issue_id(repo, number),
        repo_id: format_repository_id(repo),
        number,
        title: dto.title,
        body: dto.body,
        state,
        author_id: format_user_id(&dto.user.login),
        labels,
        assignees,
        dependencies: Vec::new(),
        version: Version::INITIAL,
        created_at: dto.created_at,
        updated_at: dto.updated_at,
        closed_at: dto.closed_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use temper_forge::UserId;

    fn repo() -> RepoCoord {
        RepoCoord::new("acme", "widgets")
    }

    #[test]
    fn maps_issue_and_sorts_collections() {
        let issue: IssueDto = serde_json::from_str(
            r#"{
                "number": 7,
                "title": "Fix bug",
                "body": "details",
                "state": "open",
                "user": {"login": "author"},
                "labels": [{"id": 2, "name": "ready"}, {"id": 1, "name": "bug"}],
                "assignees": [{"login": "carol"}, {"login": "bob"}],
                "created_at": "2024-03-01T00:00:00Z",
                "updated_at": "2024-03-02T00:00:00Z"
            }"#,
        )
        .unwrap();
        let mapped = map_issue(&repo(), issue);
        assert_eq!(mapped.id, format_issue_id(&repo(), ItemNumber::new(7)));
        assert_eq!(mapped.state, IssueState::Open);
        assert_eq!(mapped.author_id, UserId::new("author"));
        assert_eq!(mapped.labels, vec!["bug".to_string(), "ready".to_string()]);
        assert_eq!(
            mapped.assignees,
            vec![UserId::new("bob"), UserId::new("carol")]
        );
        assert!(mapped.dependencies.is_empty());
        assert_eq!(mapped.version, Version::INITIAL);

        let closed: IssueDto = serde_json::from_str(
            r#"{
                "number": 8,
                "state": "closed",
                "user": {"login": "author"},
                "labels": null,
                "assignees": null,
                "created_at": "2024-03-01T00:00:00Z",
                "updated_at": "2024-03-02T00:00:00Z",
                "closed_at": "2024-03-03T00:00:00Z"
            }"#,
        )
        .unwrap();
        let mapped = map_issue(&repo(), closed);
        assert_eq!(mapped.state, IssueState::Closed);
        assert!(mapped.labels.is_empty());
        assert!(mapped.closed_at.is_some());
    }
}
