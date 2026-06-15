//! Forgejo pull-request and review DTOs (including the head/base branch shapes).

use super::{LabelDto, UserDto};
use chrono::{DateTime, Utc};
use serde::Deserialize;

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
