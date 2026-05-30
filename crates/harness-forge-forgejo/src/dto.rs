// SPDX-License-Identifier: MPL-2.0
//! Forgejo Actions data-transfer objects for the CI adapter.
//!
//! These structs mirror the subset of Forgejo's Actions run/task payloads that
//! the CI adapter consumes. Timestamp-bearing fields decode into [`Value`] so
//! the adapter accepts either unix-seconds integers or RFC3339 strings, matching
//! the variability of the Actions API. A small pull-request detail DTO resolves
//! a pull request's head SHA/ref for run matching without depending on the
//! larger issue/pull DTOs in [`crate::types`].
use serde::Deserialize;
use serde_json::Value;

/// A Forgejo Actions workflow run.
#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct ForgejoActionRun {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub index_in_repo: i64,
    #[serde(default)]
    pub run_number: i64,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub event: String,
    #[serde(default)]
    pub prettyref: String,
    #[serde(default)]
    pub head_branch: String,
    #[serde(default)]
    pub head_sha: String,
    #[serde(default)]
    pub commit_sha: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub html_url: String,
    #[serde(default)]
    pub event_payload: String,
    #[serde(default)]
    pub created_at: Option<Value>,
    #[serde(default)]
    pub created: Option<Value>,
    #[serde(default)]
    pub updated_at: Option<Value>,
    #[serde(default)]
    pub updated: Option<Value>,
}

/// A Forgejo Actions task (a single job within a run).
#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct ForgejoActionTask {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub run_number: i64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub commit_sha: String,
    #[serde(default)]
    pub head_sha: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub html_url: String,
    #[serde(default)]
    pub created_at: Option<Value>,
    #[serde(default)]
    pub created: Option<Value>,
    #[serde(default)]
    pub updated_at: Option<Value>,
    #[serde(default)]
    pub updated: Option<Value>,
}

/// Minimal pull-request detail used to resolve a run's target head.
#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct ForgejoPrDetail {
    #[serde(default)]
    pub head: Option<ForgejoPrHead>,
}

/// The `head` block of a pull request.
#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct ForgejoPrHead {
    #[serde(default)]
    pub sha: String,
    #[serde(default, rename = "ref")]
    pub ref_name: String,
}
