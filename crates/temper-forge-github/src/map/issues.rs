//! Mapping for GitHub issues into the portable [`Issue`] model.

use super::{map_logins, normalize, sorted_dedup, sorted_dedup_users};
use crate::ids::{RepoCoord, format_issue_id, format_repository_id, format_user_id};
use crate::types::IssueDto;
use temper_forge::{Issue, IssueState, ItemNumber, Version};

/// Maps a GitHub issue DTO into a portable [`Issue`].
///
/// `version` is set to [`Version::INITIAL`]; callers that have the response
/// validator overwrite it through the backend's version cache.
/// `dependencies` stays empty: GitHub exposes no native dependency links over
/// its stable REST surface (see [`crate::dependencies`]).
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
        body: dto.body.unwrap_or_default(),
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
    fn maps_issue_with_null_body_and_sorts_collections() {
        let issue: IssueDto = serde_json::from_str(
            r#"{
                "number": 7,
                "title": "Fix bug",
                "body": null,
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
        assert_eq!(mapped.body, "");
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
