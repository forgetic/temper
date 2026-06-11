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

    /// Workflow action (intent-level tool / transition id) this job services,
    /// e.g. `open_pr` or `triage_intake`. Populated by daemon enrichment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,

    /// Checkout capability the worker should prepare for the job:
    /// `"writable"` (commit + push the head branch), `"read_only"` (analyse
    /// only; verdict result), or `"pull_request_read_only"` (read-only with
    /// the PR head fetched for diffing). Absent means writable (today's
    /// behavior).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkout_capability: Option<String>,

    /// Verdict vocabulary the job's action declares (the action's `outcomes`
    /// keys). Empty for a plain coding job.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_verdicts: Vec<String>,
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

/// One workspace-authored child issue carried by a breakdown verdict result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobChild {
    /// Stable per-child identifier within the result (seeds the child's
    /// correlation key; referenced by sibling `depends_on`).
    pub slug: String,
    pub title: String,
    pub body: String,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<Branch>,
    /// Verdict chosen by a verdict job (must be one of the assignment's
    /// `allowed_verdicts`). A success result may carry a verdict and no
    /// branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
    /// Authored body accompanying a verdict (e.g. the rewritten issue spec or
    /// the review body).
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

    fn fixture_jsons() -> Vec<(String, String)> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/reference/worker-daemon-wire-protocol/examples");
        let mut fixtures = std::fs::read_dir(dir)
            .expect("read protocol fixture directory")
            .map(|entry| entry.expect("fixture entry"))
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .and_then(|extension| extension.to_str())
                    == Some("json")
            })
            .map(|entry| {
                let path = entry.path();
                let name = path
                    .file_name()
                    .expect("fixture has a filename")
                    .to_str()
                    .expect("fixture filename is UTF-8")
                    .to_string();
                let json = std::fs::read_to_string(path).expect("fixture is readable");
                (name, json)
            })
            .collect::<Vec<_>>();
        fixtures.sort_by(|left, right| left.0.cmp(&right.0));
        fixtures
    }

    #[test]
    fn fixtures_round_trip_and_match_variants() {
        for (filename, json) in fixture_jsons() {
            let msg = assert_round_trips(&json);
            assert_eq!(protocol_version(&msg), WORKER_PROTOCOL_VERSION);

            let expected = filename.trim_end_matches(".json");
            match (expected, msg) {
                ("register", WorkerProtocolMessage::Register(_))
                | ("poll", WorkerProtocolMessage::Poll(_))
                | ("assign", WorkerProtocolMessage::Assign(_))
                | ("heartbeat", WorkerProtocolMessage::Heartbeat(_))
                | ("result", WorkerProtocolMessage::Result(_))
                | ("result-verdict", WorkerProtocolMessage::Result(_))
                | ("result-verdict-children", WorkerProtocolMessage::Result(_))
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
        assert_eq!(context.action, None);
        assert_eq!(context.checkout_capability, None);
        assert!(context.allowed_verdicts.is_empty());
    }

    #[test]
    fn old_shape_job_result_defaults_verdict_fields_to_none() {
        let result: JobResult = serde_json::from_value(serde_json::json!({
            "protocol_version": 1,
            "worker_id": "worker-1",
            "job_id": "job-123",
            "status": "success",
            "branch": {
                "name": "agent/pr-for-code-42",
                "head_sha": "0123456789abcdef0123456789abcdef01234567"
            }
        }))
        .expect("old-shape job result parses");

        assert_eq!(result.protocol_version, WORKER_PROTOCOL_VERSION);
        assert_eq!(result.worker_id, "worker-1");
        assert_eq!(result.job_id, "job-123");
        assert_eq!(result.status, ResultStatus::Success);
        assert!(result.branch.is_some());
        assert_eq!(result.verdict, None);
        assert_eq!(result.body, None);
        assert!(result.children.is_empty());
        assert_eq!(result.failure, None);
    }

    #[test]
    fn old_equivalent_job_context_omits_new_enrichment_keys() {
        let context = JobContext {
            role: "engineer".to_string(),
            repo: "ai/temper".to_string(),
            queue: "code_ready".to_string(),
            artifact_kind: "code".to_string(),
            repository: None,
            base_branch: None,
            branch_hint: None,
            correlation_key: None,
            artifact: None,
            action: None,
            checkout_capability: None,
            allowed_verdicts: Vec::new(),
        };

        assert_eq!(
            serde_json::to_value(&context).expect("job context serializes"),
            serde_json::json!({
                "role": "engineer",
                "repo": "ai/temper",
                "queue": "code_ready",
                "artifact_kind": "code"
            })
        );
    }

    #[test]
    fn old_equivalent_job_result_omits_verdict_keys() {
        let result = JobResult {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: "worker-1".to_string(),
            job_id: "job-123".to_string(),
            status: ResultStatus::Success,
            branch: Some(Branch {
                name: "agent/pr-for-code-42".to_string(),
                head_sha: "0123456789abcdef0123456789abcdef01234567".to_string(),
            }),
            verdict: None,
            body: None,
            children: Vec::new(),
            failure: None,
            summary: None,
            details: None,
        };

        let value = serde_json::to_value(&result).expect("job result serializes");
        assert_eq!(value.get("verdict"), None);
        assert_eq!(value.get("body"), None);
        assert_eq!(value.get("children"), None);
        assert_eq!(
            value,
            serde_json::json!({
                "protocol_version": 1,
                "worker_id": "worker-1",
                "job_id": "job-123",
                "status": "success",
                "branch": {
                    "name": "agent/pr-for-code-42",
                    "head_sha": "0123456789abcdef0123456789abcdef01234567"
                }
            })
        );
    }

    #[test]
    fn verdict_job_result_round_trips_without_branch() {
        let result = JobResult {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: "worker-1".to_string(),
            job_id: "job-123".to_string(),
            status: ResultStatus::Success,
            branch: None,
            verdict: Some("ready_code".to_string()),
            body: Some("Rewritten implementation-ready issue body.".to_string()),
            children: Vec::new(),
            failure: None,
            summary: Some("triaged intake".to_string()),
            details: None,
        };

        let value = serde_json::to_value(&result).expect("job result serializes");
        assert_eq!(value.get("branch"), None);
        assert_eq!(value.get("children"), None);
        assert_eq!(value["verdict"], "ready_code");
        assert_eq!(value["body"], "Rewritten implementation-ready issue body.");
        let decoded: JobResult = serde_json::from_value(value).expect("serialized result parses");

        assert_eq!(decoded, result);
    }

    #[test]
    fn verdict_job_result_round_trips_with_children() {
        let result = JobResult {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: "worker-1".to_string(),
            job_id: "job-123".to_string(),
            status: ResultStatus::Success,
            branch: None,
            verdict: Some("needs_breakdown".to_string()),
            body: None,
            children: vec![
                JobChild {
                    slug: "api-schema".to_string(),
                    title: "Define the API schema".to_string(),
                    body: "Write the shared API schema.".to_string(),
                    labels: vec!["code".to_string(), "ready".to_string()],
                    depends_on: Vec::new(),
                    target_repo: None,
                },
                JobChild {
                    slug: "web-client".to_string(),
                    title: "Implement the web client".to_string(),
                    body: "Build the web client against the API schema.".to_string(),
                    labels: vec!["code".to_string(), "ready".to_string()],
                    depends_on: vec!["api-schema".to_string()],
                    target_repo: Some("acme/web".to_string()),
                },
            ],
            failure: None,
            summary: Some("planned breakdown".to_string()),
            details: None,
        };

        let value = serde_json::to_value(&result).expect("job result serializes");
        assert_eq!(value.get("branch"), None);
        assert_eq!(value["verdict"], "needs_breakdown");
        assert_eq!(value["children"][0]["slug"], "api-schema");
        assert_eq!(
            value["children"][1]["depends_on"],
            serde_json::json!(["api-schema"])
        );
        assert_eq!(value["children"][1]["target_repo"], "acme/web");
        let decoded: JobResult = serde_json::from_value(value).expect("serialized result parses");

        assert_eq!(decoded, result);
    }

    #[test]
    fn child_defaults_omit_empty_optional_fields() {
        let child = JobChild {
            slug: "api-schema".to_string(),
            title: "Define the API schema".to_string(),
            body: "Write the shared API schema.".to_string(),
            labels: Vec::new(),
            depends_on: Vec::new(),
            target_repo: None,
        };

        assert_eq!(
            serde_json::to_value(&child).expect("child serializes"),
            serde_json::json!({
                "slug": "api-schema",
                "title": "Define the API schema",
                "body": "Write the shared API schema."
            })
        );
        assert_eq!(
            serde_json::from_value::<JobChild>(serde_json::json!({
                "slug": "api-schema",
                "title": "Define the API schema",
                "body": "Write the shared API schema."
            }))
            .expect("minimal child parses"),
            child
        );
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
            action: Some("open_pr".to_string()),
            checkout_capability: Some("writable".to_string()),
            allowed_verdicts: vec!["needs_architect".to_string()],
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
