use crate::ids::{IssueId, ItemNumber, RepositoryId, UserId};
use crate::model::common::Version;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Lifecycle state for an issue.
#[derive(Copy, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueState {
    Open,
    Closed,
}

/// Issue tracked by a repository.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Issue {
    pub id: IssueId,
    pub repo_id: RepositoryId,
    pub number: ItemNumber,
    pub title: String,
    pub body: String,
    pub state: IssueState,
    pub author_id: UserId,
    pub labels: Vec<String>,
    pub assignees: Vec<UserId>,
    /// Repository item numbers this issue depends on.
    #[serde(default)]
    pub dependencies: Vec<ItemNumber>,
    /// Optimistic-concurrency token, advanced on every successful mutation.
    #[serde(default)]
    pub version: Version,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

/// Input used to create an issue.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CreateIssue {
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub assignees: Vec<UserId>,
}

/// Partial issue update.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct UpdateIssue {
    pub title: Option<String>,
    pub body: Option<String>,
    pub state: Option<IssueState>,
    pub set_labels: Option<Vec<String>>,
    pub add_labels: Vec<String>,
    pub remove_labels: Vec<String>,
    pub add_assignees: Vec<UserId>,
    pub remove_assignees: Vec<UserId>,
    /// Optimistic-concurrency precondition. When `Some`, the update applies only
    /// if the stored [`Issue::version`] equals this token, and otherwise fails
    /// with [`ForgeError::Conflict`](crate::ForgeError::Conflict). When `None`,
    /// the update is unconditional (the backward-compatible default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<Version>,
}
