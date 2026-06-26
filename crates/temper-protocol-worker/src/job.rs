// SPDX-License-Identifier: MPL-2.0

use serde::{Deserialize, Serialize};

use crate::WorkspaceManifest;

/// Assignment-time facts for revalidating an in-flight PR-head job before it
/// publishes more work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestFreshness {
    /// Stable forge repository id captured at assignment time.
    pub repository_id: String,
    /// Human-facing repository path, used for logs and agent context.
    pub repo: String,
    pub role: String,
    pub queue: String,
    pub action: String,
    pub number: u64,
    pub pull_request_id: String,
    /// PR head SHA captured at assignment time. `None` means the forge did not
    /// expose a head SHA; a later `Some` value is therefore a mismatch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
    /// Queue condition token captured at assignment time, e.g. `ci_failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_condition: Option<String>,
    /// Queue labels that also had to match at assignment time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub queue_labels: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PullRequestFreshnessStatus {
    Fresh,
    Stale,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestFreshnessResponse {
    pub status: PullRequestFreshnessStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl PullRequestFreshnessResponse {
    pub fn fresh() -> Self {
        Self {
            status: PullRequestFreshnessStatus::Fresh,
            reason: None,
        }
    }

    pub fn stale(reason: impl Into<String>) -> Self {
        Self {
            status: PullRequestFreshnessStatus::Stale,
            reason: Some(reason.into()),
        }
    }

    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            status: PullRequestFreshnessStatus::Unavailable,
            reason: Some(reason.into()),
        }
    }
}

/// Snapshot of the Forge artifact a job acts on, taken at enqueue time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobArtifactSnapshot {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    /// Debug-formatted artifact state, e.g. `Open`.
    pub state: String,
}

/// Standard daemon-owned job payload serialized into `Assign.job_payload`.
///
/// It describes the single coordinating artifact (the issue/PR the job
/// services) plus the multi-repo [`WorkspaceManifest`] to assemble.
///
/// `artifact` and `workspace` are *enrichment* fields: the daemon maps a
/// scanned work item to a thin context first (no Forge access), then fills them
/// from Forge reads before the job is ever dispatched. They are therefore
/// `Option` on the wire DTO but **always present on a job a worker receives**;
/// the worker treats their absence as a protocol error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobContext {
    pub role: String,
    /// Primary repository path (`owner/name`) -- home of the coordinating
    /// artifact. Equal to `workspace.repos[0].repo`.
    pub repo: String,
    pub queue: String,
    pub artifact_kind: String,

    /// The coordinating artifact this job services.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<JobArtifactSnapshot>,

    /// The repositories to assemble into the job's workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceManifest>,

    /// Workflow action (intent-level tool / transition id) this job services,
    /// e.g. `open_pr` or `triage_intake`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,

    /// Checkout capability the worker should prepare for the *primary* repo:
    /// `"writable"`, `"read_only"`, `"pull_request_read_only"`, or
    /// `"pull_request_writable"`. Absent means writable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkout_capability: Option<String>,

    /// Verdict vocabulary the job's action declares (the action's `outcomes`
    /// keys). Empty for a plain coding job.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_verdicts: Vec<String>,

    /// Extra free-text guidance surfaced to the agent's prompt for this job,
    /// e.g. the concrete CI failure to fix on a `pull_request_writable` job.
    /// Absent for ordinary jobs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guidance: Option<String>,

    /// PR-head freshness guard for in-flight `pull_request_writable` jobs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_request_freshness: Option<PullRequestFreshness>,
}
