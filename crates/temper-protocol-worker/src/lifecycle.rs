// SPDX-License-Identifier: MPL-2.0

use serde::{Deserialize, Serialize};

use crate::assignment::Capability;

/// Maximum number of active model/tool operations projected in one heartbeat.
/// `active_operation_count` still reports the full parallel count.
pub const MAX_ACTIVE_OPERATION_SUMMARIES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeartbeatState {
    Running,
    Waiting,
    Finishing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobHeartbeatPhase {
    Running,
    CancelRequested,
    Quiesced,
    ResultRecorded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobOperationKind {
    Model,
    Tool,
}

/// Content-free summary of one active model/tool operation. Arguments, prompts,
/// results, and credentials are intentionally not representable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobOperationSummary {
    pub scope: String,
    pub kind: JobOperationKind,
    pub name: String,
    pub operation_id: String,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobTimeoutReason {
    NoProgress,
    MaxRun,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobTimeoutSummary {
    pub reason: JobTimeoutReason,
    pub limit_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobCancellationState {
    NotRequested,
    Requested,
    Escalated,
    Quiesced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobResultDurabilityState {
    None,
    Pending,
    Durable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobResultDeliveryState {
    NotReady,
    Pending,
}

/// Worker-owned observability projection for one attempt. This is the latest
/// report only; daemon readers must not use it as watchdog or lease authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobLiveness {
    pub phase: JobHeartbeatPhase,
    pub run_elapsed_ms: u64,
    pub no_progress_elapsed_ms: u64,
    pub active_operation_count: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub active_operations: Vec<JobOperationSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<JobTimeoutSummary>,
    pub cancellation: JobCancellationState,
    pub result_durability: JobResultDurabilityState,
    pub result_delivery: JobResultDeliveryState,
    pub pending_result: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobHeartbeat {
    pub job_id: String,
    /// Assignment fence copied from [`crate::Assign`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<String>,
    pub state: HeartbeatState,
    pub message: String,
    /// Additive structured liveness report. Legacy workers omit this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub liveness: Option<JobLiveness>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Heartbeat {
    pub protocol_version: u32,
    pub worker_id: String,
    pub jobs: Vec<JobHeartbeat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_capacity: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_pool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent_jobs: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<Capability>,
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
    /// Fence acknowledged by this release. Legacy releases may omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<String>,
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
    Unauthorized,
    RegistrationRejected,
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
