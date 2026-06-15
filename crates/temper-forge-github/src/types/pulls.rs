//! DTOs for GitHub pull requests, reviews, and the merge result.

use super::{LabelDto, UserDto};
use chrono::{DateTime, Utc};
use serde::Deserialize;

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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn deserializes_merge_result() {
        let merge: MergeResultDto = serde_json::from_str(
            r#"{"sha": "mergesha", "merged": true, "message": "Pull Request successfully merged"}"#,
        )
        .unwrap();
        assert!(merge.merged);
        assert_eq!(merge.sha.as_deref(), Some("mergesha"));
    }
}
