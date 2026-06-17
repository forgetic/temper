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
/// sha) -- high-frequency observability belongs to the out-of-band control
/// plane and must not grow fields here.
///
/// There is deliberately no `job_id`: the workspace `correlation_key` (the
/// manifest's `coordination_key` value) is the one cross-plane identifier, and
/// the daemon resolves it to its in-flight job. Delivery is fire-and-forget and
/// the daemon applies progress idempotently keyed by
/// `(correlation_key, step, state)`, so re-delivery after worker retry or
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
