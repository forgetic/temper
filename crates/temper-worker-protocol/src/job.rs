// SPDX-License-Identifier: MPL-2.0

use serde::{Deserialize, Serialize};

use crate::WorkspaceManifest;

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
    /// `"writable"`, `"read_only"`, or `"pull_request_read_only"`. Absent means
    /// writable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkout_capability: Option<String>,

    /// Verdict vocabulary the job's action declares (the action's `outcomes`
    /// keys). Empty for a plain coding job.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_verdicts: Vec<String>,
}
