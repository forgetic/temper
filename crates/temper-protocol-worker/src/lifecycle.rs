// SPDX-License-Identifier: MPL-2.0

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeartbeatState {
    Running,
    Waiting,
    Finishing,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobHeartbeat {
    pub job_id: String,
    pub state: HeartbeatState,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Heartbeat {
    pub protocol_version: u32,
    pub worker_id: String,
    pub jobs: Vec<JobHeartbeat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_capacity: Option<u32>,
}

/// One agent step-progress checkpoint, relayed worker -> daemon.
///
/// Carries the agent protocol's `StepProgress` fields plus the message
/// envelope. Per the agent/orchestration split's bright-line rule, this is
/// durable human-facing PR state only (step label, lifecycle phase, pushed
/// sha, optional plan publication data) -- high-frequency observability belongs
/// to the out-of-band control plane and must not grow fields here.
///
/// There is deliberately no `job_id`: the workspace `correlation_key` (the
/// manifest's `coordination_key` value) is the one cross-plane identifier, and
/// the daemon resolves it to its in-flight job. Delivery is fire-and-forget and
/// the daemon applies progress idempotently to the implementation PR checklist
/// by `(correlation_key, status, state)` when `status` matches a plan phase;
/// terminal final-summary comments, when emitted, keep a
/// `(correlation_key, step, state)` marker so re-delivery after worker retry or
/// daemon restart is safe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobProgress {
    pub protocol_version: u32,
    pub worker_id: String,
    /// The job's coordination key (echoes `WorkspaceManifest.coordination_key`).
    pub correlation_key: String,
    /// Monotonic step index from the agent, starting at 1.
    pub step: u32,
    /// Short imperative step label, e.g. "write failing test".
    pub status: String,
    /// Lifecycle phase: `"started"` or `"done"`.
    pub state: String,
    /// Commit sha the step pushed, when it pushed one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pushed_sha: Option<String>,
    /// Optional one-line human note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Optional model-authored plan publication, with host-filled repo routing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_publication: Option<JobPlanPublication>,
}

/// Plan publication data relayed worker -> daemon on a progress message.
///
/// Mirrors the agent protocol's plan-publication shape without making the
/// worker/daemon protocol crate depend on the agent protocol crate; CI enforces
/// that this DTO crate stays dependency-light.
#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct JobPlanPublication {
    /// Short human summary or title for the planned change.
    pub summary: String,
    /// Ordered human-readable phase labels.
    #[serde(default)]
    pub phases: Vec<String>,
    /// Target repositories the plan applies to, in workspace/manifest order.
    #[serde(default)]
    pub target_repos: Vec<JobPlanPublicationTarget>,
}

/// One repository target included in a [`JobPlanPublication`].
#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct JobPlanPublicationTarget {
    /// Repository path in `owner/name` form.
    pub repo_path: String,
    /// Workspace-relative checkout directory for the repository.
    pub dir: String,
    /// Branch the work is based on.
    pub base_branch: String,
    /// Host-provided work branch hint, when the target is writable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_hint: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseDisposition {
    Accepted,
    Superseded,
    Reclaimed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Release {
    pub protocol_version: u32,
    pub worker_id: String,
    pub job_id: String,
    pub disposition: ReleaseDisposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseAckDisposition {
    Released,
    UnknownJob,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LeaseAck {
    pub protocol_version: u32,
    pub worker_id: String,
    pub job_id: String,
    pub disposition: LeaseAckDisposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    PollTimeout,
    ProtocolVersionMismatch,
    MalformedMessage,
    UnknownWorker,
    CapacityExceeded,
    HeartbeatMissed,
    JobTimeout,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProtocolError {
    pub protocol_version: u32,
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
}
