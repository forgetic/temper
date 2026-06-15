//! Forgejo issue-row DTOs (issues, the PR-as-issue marker, dependency refs).

use super::{LabelDto, UserDto};
use chrono::{DateTime, Utc};
use serde::Deserialize;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
