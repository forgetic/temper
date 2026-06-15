//! DTOs for GitHub issues, including the PR-as-issue marker.

use super::{LabelDto, UserDto};
use chrono::{DateTime, Utc};
use serde::Deserialize;

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
                "pull_request": {"url": "https://api.github.com/repos/acme/widgets/pulls/8", "merged_at": null}
            }"#,
        )
        .unwrap();
        assert!(pull_as_issue.is_pull_request());
    }
}
