// SPDX-License-Identifier: MPL-2.0

//! Pure daemon-side Worker Protocol handling.
//!
//! `DaemonCore` maps already-received worker protocol DTOs to the in-memory
//! dispatch coordinator and returns response DTOs. It intentionally performs no
//! networking, async work, I/O, clock reads, sleeps, or transport-level
//! long-poll waiting; callers are responsible for transport behavior.

use std::collections::BTreeMap;

use temper_worker_protocol::{
    Artifact, Assign, ErrorCode, Heartbeat, JobResult, LeaseAck, Poll, ProtocolError, Register,
    Release, ReleaseDisposition, WorkerProtocolMessage, WORKER_PROTOCOL_VERSION,
};

use crate::{DispatchCoordinator, WorkItem};

#[derive(Debug, Clone, PartialEq)]
pub struct QueuedJob {
    pub job_id: String,
    pub role: String,
    pub repo: String,
    pub artifact: Artifact,
    pub job_payload: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InFlightJob {
    pub job_id: String,
    pub role: String,
    pub repo: String,
    pub artifact: Artifact,
    pub job_payload: serde_json::Value,
}

#[derive(Debug, Default)]
pub struct DaemonCore {
    coordinator: DispatchCoordinator,
    job_context: BTreeMap<String, (Artifact, serde_json::Value)>,
}

impl DaemonCore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn coordinator(&self) -> &DispatchCoordinator {
        &self.coordinator
    }

    pub fn coordinator_mut(&mut self) -> &mut DispatchCoordinator {
        &mut self.coordinator
    }

    pub fn enqueue_job(
        &mut self,
        job_id: impl Into<String>,
        role: impl Into<String>,
        repo: impl Into<String>,
        artifact: Artifact,
        job_payload: serde_json::Value,
    ) {
        let job_id = job_id.into();
        let role = role.into();
        let repo = repo.into();
        let repos = manifest_repos(&job_payload, &repo);

        self.coordinator.enqueue(WorkItem {
            job_id: job_id.clone(),
            role,
            repo,
            repos,
        });
        self.job_context.insert(job_id, (artifact, job_payload));
    }

    pub fn queued_jobs(&self) -> Vec<QueuedJob> {
        self.coordinator
            .pending()
            .iter()
            .filter_map(|item| {
                let (artifact, job_payload) = self.job_context.get(&item.job_id)?.clone();
                Some(QueuedJob {
                    job_id: item.job_id.clone(),
                    role: item.role.clone(),
                    repo: item.repo.clone(),
                    artifact,
                    job_payload,
                })
            })
            .collect()
    }

    /// Full context of a currently in-flight (assigned, not yet completed) job,
    /// recoverable until `handle(Result)` completes it. `None` if the job is
    /// pending (not yet dispatched), unknown, or already completed.
    pub fn in_flight_job(&self, job_id: &str) -> Option<InFlightJob> {
        let item = self.coordinator.assigned_work_item(job_id)?;
        let (artifact, job_payload) = self.job_context.get(job_id)?.clone();
        Some(InFlightJob {
            job_id: job_id.to_string(),
            role: item.role.clone(),
            repo: item.repo.clone(),
            artifact,
            job_payload,
        })
    }

    /// The in-flight job whose payload carries this workspace correlation key
    /// (the one cross-plane identifier; step-progress messages are keyed by
    /// it rather than `job_id`). `None` when no assigned job matches.
    pub fn in_flight_job_by_correlation_key(&self, correlation_key: &str) -> Option<InFlightJob> {
        self.job_context.iter().find_map(|(job_id, (_, payload))| {
            (payload_coordination_key(payload) == Some(correlation_key))
                .then(|| self.in_flight_job(job_id))
                .flatten()
        })
    }

    pub fn handle(&mut self, msg: WorkerProtocolMessage) -> Option<WorkerProtocolMessage> {
        if protocol_version(&msg) != WORKER_PROTOCOL_VERSION {
            return Some(error(
                ErrorCode::ProtocolVersionMismatch,
                "unsupported protocol_version",
                None,
            ));
        }

        match msg {
            WorkerProtocolMessage::Register(register) => self.handle_register(register),
            WorkerProtocolMessage::Poll(poll) => Some(self.handle_poll(poll)),
            WorkerProtocolMessage::Assign(_) | WorkerProtocolMessage::Release(_) => Some(error(
                ErrorCode::MalformedMessage,
                "daemon-to-worker message received inbound",
                None,
            )),
            WorkerProtocolMessage::Heartbeat(heartbeat) => self.handle_heartbeat(heartbeat),
            WorkerProtocolMessage::Result(result) => Some(self.handle_result(result)),
            WorkerProtocolMessage::LeaseAck(lease_ack) => self.handle_lease_ack(lease_ack),
            // Progress is observability bookkeeping the daemon machine routes
            // to the forge applier before reaching the core; no reply either
            // way (fire-and-forget by contract).
            WorkerProtocolMessage::Progress(_) => None,
            WorkerProtocolMessage::Error(_) => None,
        }
    }

    fn handle_register(&mut self, register: Register) -> Option<WorkerProtocolMessage> {
        self.coordinator.register(&register);
        None
    }

    fn handle_poll(&mut self, poll: Poll) -> WorkerProtocolMessage {
        if !self.coordinator.registry().is_healthy(&poll.worker_id) {
            return error(ErrorCode::UnknownWorker, "unknown worker", None);
        }

        let Some(assignment) = self.coordinator.dispatch_for_worker(&poll.worker_id) else {
            return error(ErrorCode::PollTimeout, "no work available", None);
        };

        let Some((artifact, job_payload)) = self.job_context.get(&assignment.job_id).cloned()
        else {
            let _ = self.coordinator.complete(&assignment.job_id);
            return error(
                ErrorCode::MalformedMessage,
                "assigned job missing daemon job context",
                Some(assignment.job_id),
            );
        };

        WorkerProtocolMessage::Assign(Assign {
            protocol_version: WORKER_PROTOCOL_VERSION,
            job_id: assignment.job_id,
            role: assignment.role,
            repo: assignment.repo,
            artifact,
            job_payload,
        })
    }

    fn handle_heartbeat(&mut self, heartbeat: Heartbeat) -> Option<WorkerProtocolMessage> {
        match self
            .coordinator
            .registry_mut()
            .heartbeat(&heartbeat.worker_id)
        {
            Ok(()) => None,
            Err(_) => Some(error(ErrorCode::UnknownWorker, "unknown worker", None)),
        }
    }

    fn handle_result(&mut self, result: JobResult) -> WorkerProtocolMessage {
        let _ = self.coordinator.complete(&result.job_id);
        self.job_context.remove(&result.job_id);

        WorkerProtocolMessage::Release(Release {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: result.worker_id,
            job_id: result.job_id,
            disposition: ReleaseDisposition::Accepted,
            message: None,
        })
    }

    fn handle_lease_ack(&mut self, _lease_ack: LeaseAck) -> Option<WorkerProtocolMessage> {
        None
    }
}

/// Every repository the assigned worker must be capable of: the manifest's
/// `workspace.repos[*].repo` (ADR 0023), falling back to just the primary repo
/// when the payload carries no manifest. Non-empty by construction.
fn manifest_repos(job_payload: &serde_json::Value, primary: &str) -> Vec<String> {
    let repos = job_payload
        .get("workspace")
        .and_then(|workspace| workspace.get("repos"))
        .and_then(serde_json::Value::as_array)
        .map(|repos| {
            repos
                .iter()
                .filter_map(|repo| repo.get("repo").and_then(serde_json::Value::as_str))
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if repos.is_empty() {
        vec![primary.to_string()]
    } else {
        repos
    }
}

/// The job's coordination key (`workspace.coordination_key`), the one
/// cross-plane identifier step-progress is keyed by.
fn payload_coordination_key(job_payload: &serde_json::Value) -> Option<&str> {
    job_payload
        .get("workspace")
        .and_then(|workspace| workspace.get("coordination_key"))
        .and_then(serde_json::Value::as_str)
}

fn protocol_version(msg: &WorkerProtocolMessage) -> u32 {
    match msg {
        WorkerProtocolMessage::Register(msg) => msg.protocol_version,
        WorkerProtocolMessage::Poll(msg) => msg.protocol_version,
        WorkerProtocolMessage::Assign(msg) => msg.protocol_version,
        WorkerProtocolMessage::Heartbeat(msg) => msg.protocol_version,
        WorkerProtocolMessage::Result(msg) => msg.protocol_version,
        WorkerProtocolMessage::Progress(msg) => msg.protocol_version,
        WorkerProtocolMessage::Release(msg) => msg.protocol_version,
        WorkerProtocolMessage::LeaseAck(msg) => msg.protocol_version,
        WorkerProtocolMessage::Error(msg) => msg.protocol_version,
    }
}

fn error(code: ErrorCode, message: &str, job_id: Option<String>) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Error(ProtocolError {
        protocol_version: WORKER_PROTOCOL_VERSION,
        code,
        message: message.to_string(),
        retry_after_ms: None,
        job_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use temper_worker_protocol::{Capability, Capacity, ResultStatus};

    fn register(worker_id: &str, role: &str, repo: &str, max_concurrent_jobs: u32) -> Register {
        Register {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: worker_id.to_string(),
            capabilities: vec![Capability {
                role: role.to_string(),
                repo: repo.to_string(),
            }],
            capacity: Capacity {
                max_concurrent_jobs,
            },
            labels: None,
        }
    }

    fn poll(worker_id: &str) -> WorkerProtocolMessage {
        WorkerProtocolMessage::Poll(Poll {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: worker_id.to_string(),
            free_capacity: 1,
            max_wait_ms: Some(30_000),
        })
    }

    fn heartbeat(worker_id: &str) -> WorkerProtocolMessage {
        WorkerProtocolMessage::Heartbeat(Heartbeat {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: worker_id.to_string(),
            jobs: Vec::new(),
            free_capacity: Some(1),
        })
    }

    fn artifact() -> Artifact {
        Artifact {
            item: json!(99),
            kind: "issue".to_string(),
        }
    }

    fn result(worker_id: &str, job_id: &str) -> WorkerProtocolMessage {
        WorkerProtocolMessage::Result(JobResult {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: worker_id.to_string(),
            job_id: job_id.to_string(),
            status: ResultStatus::Success,
            repos: Vec::new(),
            verdict: None,
            body: None,
            children: Vec::new(),
            failure: None,
            summary: Some("done".to_string()),
            details: None,
        })
    }

    fn assert_error(msg: Option<WorkerProtocolMessage>, code: ErrorCode, message: &str) {
        match msg {
            Some(WorkerProtocolMessage::Error(error)) => {
                assert_eq!(error.code, code);
                assert_eq!(error.message, message);
            }
            other => panic!("expected protocol error, got {other:?}"),
        }
    }

    fn register_multi(
        worker_id: &str,
        role: &str,
        repos: &[&str],
        max_concurrent_jobs: u32,
    ) -> Register {
        Register {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: worker_id.to_string(),
            capabilities: repos
                .iter()
                .map(|repo| Capability {
                    role: role.to_string(),
                    repo: (*repo).to_string(),
                })
                .collect(),
            capacity: Capacity {
                max_concurrent_jobs,
            },
            labels: None,
        }
    }

    fn coordinated_payload(coordination_key: &str, repos: &[&str]) -> serde_json::Value {
        let repos = repos
            .iter()
            .map(|repo| {
                json!({
                    "repo": repo,
                    "dir": repo.split('/').next_back().unwrap(),
                    "access": "writable",
                    "default_branch": "main",
                    "base_branch": "main",
                    "branch_hint": format!("agent/{coordination_key}"),
                })
            })
            .collect::<Vec<_>>();
        json!({ "workspace": { "coordination_key": coordination_key, "repos": repos } })
    }

    #[test]
    fn coordinated_job_dispatches_only_to_an_all_repo_capable_worker() {
        let mut core = DaemonCore::new();
        let payload = coordinated_payload("coord-1", &["ai/temper", "ai/smith", "ai/skein"]);
        core.enqueue_job("job-coord", "engineer", "ai/temper", artifact(), payload);

        // Capable of two of the three manifest repos: no assignment.
        core.coordinator_mut()
            .register(&register_multi("partial", "engineer", &["ai/temper", "ai/smith"], 1));
        assert_error(
            core.handle(poll("partial")),
            ErrorCode::PollTimeout,
            "no work available",
        );

        // Capable of all three: gets the assign, primary repo on the envelope.
        core.coordinator_mut().register(&register_multi(
            "full",
            "engineer",
            &["ai/temper", "ai/smith", "ai/skein"],
            1,
        ));
        match core.handle(poll("full")) {
            Some(WorkerProtocolMessage::Assign(assign)) => {
                assert_eq!(assign.job_id, "job-coord");
                assert_eq!(assign.repo, "ai/temper");
            }
            other => panic!("expected assign, got {other:?}"),
        }

        // The in-flight job resolves by its workspace coordination key.
        let resolved = core
            .in_flight_job_by_correlation_key("coord-1")
            .expect("coordination key resolves to the in-flight job");
        assert_eq!(resolved.job_id, "job-coord");
    }

    #[test]
    fn assigned_job_is_recoverable_as_in_flight() {
        let mut core = DaemonCore::new();
        core.coordinator_mut()
            .register(&register("worker-a", "engineer", "ai/temper", 1));
        let artifact = artifact();
        let payload = json!({"k":1});
        core.enqueue_job(
            "job-1",
            "engineer",
            "ai/temper",
            artifact.clone(),
            payload.clone(),
        );

        match core.handle(poll("worker-a")) {
            Some(WorkerProtocolMessage::Assign(assign)) => assert_eq!(assign.job_id, "job-1"),
            other => panic!("expected assign, got {other:?}"),
        }

        assert_eq!(
            core.in_flight_job("job-1"),
            Some(InFlightJob {
                job_id: "job-1".to_string(),
                role: "engineer".to_string(),
                repo: "ai/temper".to_string(),
                artifact,
                job_payload: payload,
            })
        );
    }

    #[test]
    fn pending_job_is_not_in_flight() {
        let mut core = DaemonCore::new();
        core.enqueue_job("job-1", "engineer", "ai/temper", artifact(), json!({"k":1}));

        assert_eq!(core.in_flight_job("job-1"), None);
    }

    #[test]
    fn completed_job_is_no_longer_in_flight() {
        let mut core = DaemonCore::new();
        core.coordinator_mut()
            .register(&register("worker-a", "engineer", "ai/temper", 1));
        core.enqueue_job("job-1", "engineer", "ai/temper", artifact(), json!({"k":1}));

        match core.handle(poll("worker-a")) {
            Some(WorkerProtocolMessage::Assign(assign)) => assert_eq!(assign.job_id, "job-1"),
            other => panic!("expected assign, got {other:?}"),
        }
        let _ = core.handle(result("worker-a", "job-1"));

        assert_eq!(core.in_flight_job("job-1"), None);
    }

    #[test]
    fn unknown_job_is_not_in_flight_or_assigned() {
        let core = DaemonCore::new();

        assert_eq!(core.in_flight_job("nope"), None);
        assert_eq!(core.coordinator().assigned_work_item("nope"), None);
    }

    #[test]
    fn register_then_poll_returns_assign_with_job_context() {
        let mut core = DaemonCore::new();
        let artifact = artifact();
        let payload = json!({"prompt":"implement"});
        core.enqueue_job(
            "job-1",
            "engineer",
            "ai/temper",
            artifact.clone(),
            payload.clone(),
        );
        assert_eq!(
            core.handle(WorkerProtocolMessage::Register(register(
                "worker-a",
                "engineer",
                "ai/temper",
                1,
            ))),
            None
        );

        match core.handle(poll("worker-a")) {
            Some(WorkerProtocolMessage::Assign(assign)) => {
                assert_eq!(assign.job_id, "job-1");
                assert_eq!(assign.role, "engineer");
                assert_eq!(assign.repo, "ai/temper");
                assert_eq!(assign.artifact, artifact);
                assert_eq!(assign.job_payload, payload);
            }
            other => panic!("expected assign, got {other:?}"),
        }
    }

    #[test]
    fn poll_with_no_work_returns_poll_timeout_error() {
        let mut core = DaemonCore::new();
        core.coordinator_mut()
            .register(&register("worker-a", "engineer", "ai/temper", 1));

        assert_error(
            core.handle(poll("worker-a")),
            ErrorCode::PollTimeout,
            "no work available",
        );
    }

    #[test]
    fn poll_from_unknown_worker_returns_unknown_worker_error() {
        let mut core = DaemonCore::new();
        assert_error(
            core.handle(poll("missing")),
            ErrorCode::UnknownWorker,
            "unknown worker",
        );
    }

    #[test]
    fn poll_only_returns_capability_matching_work() {
        let mut core = DaemonCore::new();
        core.enqueue_job("job-1", "architect", "ai/temper", artifact(), json!({}));
        core.coordinator_mut()
            .register(&register("engineer-a", "engineer", "ai/temper", 1));

        assert_error(
            core.handle(poll("engineer-a")),
            ErrorCode::PollTimeout,
            "no work available",
        );

        core.coordinator_mut()
            .register(&register("architect-a", "architect", "ai/temper", 1));
        match core.handle(poll("architect-a")) {
            Some(WorkerProtocolMessage::Assign(assign)) => assert_eq!(assign.job_id, "job-1"),
            other => panic!("expected assign, got {other:?}"),
        }
    }

    #[test]
    fn heartbeat_known_worker_returns_none_and_unknown_returns_error() {
        let mut core = DaemonCore::new();
        core.coordinator_mut()
            .register(&register("worker-a", "engineer", "ai/temper", 1));

        assert_eq!(core.handle(heartbeat("worker-a")), None);
        assert_error(
            core.handle(heartbeat("missing")),
            ErrorCode::UnknownWorker,
            "unknown worker",
        );
    }

    #[test]
    fn result_returns_release_accepted_and_frees_capacity() {
        let mut core = DaemonCore::new();
        core.coordinator_mut()
            .register(&register("worker-a", "engineer", "ai/temper", 1));
        core.enqueue_job("job-1", "engineer", "ai/temper", artifact(), json!({"n":1}));
        core.enqueue_job("job-2", "engineer", "ai/temper", artifact(), json!({"n":2}));

        match core.handle(poll("worker-a")) {
            Some(WorkerProtocolMessage::Assign(assign)) => assert_eq!(assign.job_id, "job-1"),
            other => panic!("expected first assign, got {other:?}"),
        }

        match core.handle(result("worker-a", "job-1")) {
            Some(WorkerProtocolMessage::Release(release)) => {
                assert_eq!(release.worker_id, "worker-a");
                assert_eq!(release.job_id, "job-1");
                assert_eq!(release.disposition, ReleaseDisposition::Accepted);
            }
            other => panic!("expected release, got {other:?}"),
        }

        match core.handle(poll("worker-a")) {
            Some(WorkerProtocolMessage::Assign(assign)) => assert_eq!(assign.job_id, "job-2"),
            other => panic!("expected second assign, got {other:?}"),
        }
    }

    #[test]
    fn version_mismatch_returns_protocol_version_mismatch_error() {
        let mut core = DaemonCore::new();
        let mut register = register("worker-a", "engineer", "ai/temper", 1);
        register.protocol_version = WORKER_PROTOCOL_VERSION + 1;

        assert_error(
            core.handle(WorkerProtocolMessage::Register(register)),
            ErrorCode::ProtocolVersionMismatch,
            "unsupported protocol_version",
        );
    }

    #[test]
    fn inbound_assign_or_release_is_malformed_message() {
        let mut core = DaemonCore::new();
        let assign = WorkerProtocolMessage::Assign(Assign {
            protocol_version: WORKER_PROTOCOL_VERSION,
            job_id: "job-1".to_string(),
            role: "engineer".to_string(),
            repo: "ai/temper".to_string(),
            artifact: artifact(),
            job_payload: json!({}),
        });
        let release = WorkerProtocolMessage::Release(Release {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: "worker-a".to_string(),
            job_id: "job-1".to_string(),
            disposition: ReleaseDisposition::Accepted,
            message: None,
        });

        assert_error(
            core.handle(assign),
            ErrorCode::MalformedMessage,
            "daemon-to-worker message received inbound",
        );
        assert_error(
            core.handle(release),
            ErrorCode::MalformedMessage,
            "daemon-to-worker message received inbound",
        );
    }
}
