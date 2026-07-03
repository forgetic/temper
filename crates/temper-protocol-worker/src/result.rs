// SPDX-License-Identifier: MPL-2.0

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultStatus {
    Success,
    Failure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    Transient,
    Permanent,
    Canceled,
    Protocol,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Branch {
    pub name: String,
    pub head_sha: String,
}

/// The pushed product of one writable repository in a coordinated head result.
/// The daemon opens one pull request per outcome.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepoOutcome {
    /// Repository path, `owner/name`.
    pub repo: String,
    pub branch: Branch,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Failure {
    pub class: FailureClass,
    pub message: String,
}

/// One workspace-authored child issue carried by a breakdown verdict result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobChild {
    /// Stable per-child identifier within the result (seeds the child's
    /// correlation key; referenced by sibling `depends_on`).
    pub slug: String,
    pub title: String,
    pub body: String,
    /// Workflow artifact kind for this child issue. Omitted defaults to `code`
    /// when the daemon applies verdict child fan-out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    /// Slugs of sibling children in the same result that must land before
    /// this one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    /// Target repository as an `owner/name` path (the same shape the daemon's
    /// `--repo` flag parses). `None` = the job's own repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_repo: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobResult {
    pub protocol_version: u32,
    pub worker_id: String,
    pub job_id: String,
    pub status: ResultStatus,
    /// Per-repo head products -- one per writable repo that produced a diff. The
    /// daemon opens one pull request per entry. Empty for a verdict result.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repos: Vec<RepoOutcome>,
    /// Verdict chosen by a verdict job (must be one of the assignment's
    /// `allowed_verdicts`). A success result may carry a verdict and no repos.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
    /// Agent-authored implementation PR title for a no-verdict success. Ignored
    /// for verdict results.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Authored body. With a verdict, this is the verdict payload (e.g. the
    /// rewritten issue spec or the review body). Without a verdict, this is the
    /// implementation PR report body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Child issues authored by a breakdown verdict (e.g. `needs_breakdown`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<JobChild>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<Failure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}
