// SPDX-License-Identifier: MPL-2.0

//! Serialization-only DTOs for the Temper Worker/Daemon Wire Protocol v1.
//!
//! This crate intentionally has no Temper runtime dependencies. It provides the
//! stable JSON shapes that workers and daemons can share without coupling Smith
//! or other worker implementations to Temper runner, workflow, backend, daemon,
//! deployment, or Forge crates.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const WORKER_PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum WorkerProtocolMessage {
    Register(Register),
    Poll(Poll),
    Assign(Assign),
    Heartbeat(Heartbeat),
    Result(JobResult),
    Release(Release),
    LeaseAck(LeaseAck),
    Error(ProtocolError),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Capability {
    pub role: String,
    pub repo: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Capacity {
    pub max_concurrent_jobs: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Register {
    pub protocol_version: u32,
    pub worker_id: String,
    pub capabilities: Vec<Capability>,
    pub capacity: Capacity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Poll {
    pub protocol_version: u32,
    pub worker_id: String,
    pub free_capacity: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_wait_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Artifact {
    /// String or numeric artifact identity, preserved as JSON per the protocol.
    pub item: Value,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Assign {
    pub protocol_version: u32,
    pub job_id: String,
    pub role: String,
    pub repo: String,
    pub artifact: Artifact,
    pub job_payload: Value,
}

/// Repository coordinates for a job assignment (standard job payload v1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobRepository {
    pub owner: String,
    pub name: String,
    pub default_branch: String,
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
/// Enrichment fields are optional so older minimal payloads still parse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobContext {
    pub role: String,
    pub repo: String,
    pub queue: String,
    pub artifact_kind: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<JobRepository>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,

    /// Branch the worker should push, e.g. `agent/pr-for-code-42`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_hint: Option<String>,

    /// Idempotent PR correlation key, e.g. `pr-for-code-42`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_key: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<JobArtifactSnapshot>,
}

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Failure {
    pub class: FailureClass,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobResult {
    pub protocol_version: u32,
    pub worker_id: String,
    pub job_id: String,
    pub status: ResultStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<Branch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<Failure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
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

#[cfg(test)]
mod tests {
    use super::*;

    const REGISTER: &str =
        include_str!("../../../docs/reference/worker-daemon-wire-protocol/examples/register.json");
    const POLL: &str =
        include_str!("../../../docs/reference/worker-daemon-wire-protocol/examples/poll.json");
    const ASSIGN: &str =
        include_str!("../../../docs/reference/worker-daemon-wire-protocol/examples/assign.json");
    const HEARTBEAT: &str =
        include_str!("../../../docs/reference/worker-daemon-wire-protocol/examples/heartbeat.json");
    const RESULT: &str =
        include_str!("../../../docs/reference/worker-daemon-wire-protocol/examples/result.json");
    const RELEASE: &str =
        include_str!("../../../docs/reference/worker-daemon-wire-protocol/examples/release.json");
    const LEASE_ACK: &str =
        include_str!("../../../docs/reference/worker-daemon-wire-protocol/examples/lease-ack.json");
    const ERROR: &str =
        include_str!("../../../docs/reference/worker-daemon-wire-protocol/examples/error.json");

    fn assert_round_trips(json: &str) -> WorkerProtocolMessage {
        let msg: WorkerProtocolMessage = serde_json::from_str(json).expect("fixture parses");
        let encoded = serde_json::to_string(&msg).expect("serializes");
        let again: WorkerProtocolMessage = serde_json::from_str(&encoded).expect("round-trips");
        assert_eq!(msg, again, "round-trip must be lossless");
        msg
    }

    fn protocol_version(msg: &WorkerProtocolMessage) -> u32 {
        match msg {
            WorkerProtocolMessage::Register(msg) => msg.protocol_version,
            WorkerProtocolMessage::Poll(msg) => msg.protocol_version,
            WorkerProtocolMessage::Assign(msg) => msg.protocol_version,
            WorkerProtocolMessage::Heartbeat(msg) => msg.protocol_version,
            WorkerProtocolMessage::Result(msg) => msg.protocol_version,
            WorkerProtocolMessage::Release(msg) => msg.protocol_version,
            WorkerProtocolMessage::LeaseAck(msg) => msg.protocol_version,
            WorkerProtocolMessage::Error(msg) => msg.protocol_version,
        }
    }

    #[test]
    fn fixtures_round_trip_and_match_variants() {
        let fixtures = [
            (REGISTER, "register"),
            (POLL, "poll"),
            (ASSIGN, "assign"),
            (HEARTBEAT, "heartbeat"),
            (RESULT, "result"),
            (RELEASE, "release"),
            (LEASE_ACK, "lease-ack"),
            (ERROR, "error"),
        ];

        for (json, name) in fixtures {
            let msg = assert_round_trips(json);
            assert_eq!(protocol_version(&msg), WORKER_PROTOCOL_VERSION);

            match (name, msg) {
                ("register", WorkerProtocolMessage::Register(_))
                | ("poll", WorkerProtocolMessage::Poll(_))
                | ("assign", WorkerProtocolMessage::Assign(_))
                | ("heartbeat", WorkerProtocolMessage::Heartbeat(_))
                | ("result", WorkerProtocolMessage::Result(_))
                | ("release", WorkerProtocolMessage::Release(_))
                | ("lease-ack", WorkerProtocolMessage::LeaseAck(_))
                | ("error", WorkerProtocolMessage::Error(_)) => {}
                (name, msg) => panic!("{name} parsed as unexpected variant: {msg:?}"),
            }
        }
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let msg: WorkerProtocolMessage = serde_json::from_str(
            r#"{"type":"poll","protocol_version":1,"worker_id":"w1","free_capacity":2,"future_field":"ignored"}"#,
        )
        .expect("unknown fields must be accepted");

        assert!(matches!(msg, WorkerProtocolMessage::Poll(_)));
    }

    #[test]
    fn minimal_job_context_defaults_enrichment_fields_to_none() {
        let context: JobContext = serde_json::from_value(serde_json::json!({
            "role": "engineer",
            "repo": "ai/temper",
            "queue": "code_ready",
            "artifact_kind": "code"
        }))
        .expect("minimal job context parses");

        assert_eq!(context.role, "engineer");
        assert_eq!(context.repo, "ai/temper");
        assert_eq!(context.queue, "code_ready");
        assert_eq!(context.artifact_kind, "code");
        assert_eq!(context.repository, None);
        assert_eq!(context.base_branch, None);
        assert_eq!(context.branch_hint, None);
        assert_eq!(context.correlation_key, None);
        assert_eq!(context.artifact, None);
    }

    #[test]
    fn full_job_context_round_trips_without_loss() {
        let context = JobContext {
            role: "engineer".to_string(),
            repo: "ai/temper".to_string(),
            queue: "code_ready".to_string(),
            artifact_kind: "code".to_string(),
            repository: Some(JobRepository {
                owner: "ai".to_string(),
                name: "temper".to_string(),
                default_branch: "main".to_string(),
            }),
            base_branch: Some("main".to_string()),
            branch_hint: Some("agent/pr-for-code-42".to_string()),
            correlation_key: Some("pr-for-code-42".to_string()),
            artifact: Some(JobArtifactSnapshot {
                number: 42,
                title: "Implement daemon payload".to_string(),
                body: "Add the standard job context.".to_string(),
                labels: vec!["code".to_string(), "ready".to_string()],
                state: "Open".to_string(),
            }),
        };

        let value = serde_json::to_value(&context).expect("job context serializes");
        let decoded: JobContext =
            serde_json::from_value(value).expect("serialized job context parses");

        assert_eq!(decoded, context);
    }

    #[test]
    fn job_context_unknown_fields_are_ignored() {
        let context: JobContext = serde_json::from_value(serde_json::json!({
            "role": "engineer",
            "repo": "ai/temper",
            "queue": "code_ready",
            "artifact_kind": "code",
            "future_field": "ignored"
        }))
        .expect("unknown job context fields must be accepted");

        assert_eq!(context.role, "engineer");
        assert_eq!(context.repository, None);
    }
}
