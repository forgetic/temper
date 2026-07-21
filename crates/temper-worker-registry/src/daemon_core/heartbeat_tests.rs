// SPDX-License-Identifier: MPL-2.0

use serde_json::json;
use temper_protocol_worker::{
    Heartbeat, HeartbeatState, JobHeartbeat, WORKER_PROTOCOL_VERSION, WorkerProtocolMessage,
};

use super::{DaemonCore, RecoveredJob};
use crate::test_support::{artifact, coordinated_payload, poll, register};

#[test]
fn matching_heartbeat_reattaches_staged_assignment_and_rejects_other_reports() {
    let mut core = DaemonCore::new();
    core.stage_recovered_job(RecoveredJob {
        job_id: "job-old".to_string(),
        attempt_id: Some("attempt-old".to_string()),
        worker_id: "worker-a".to_string(),
        role: "engineer".to_string(),
        repo: "ai/temper".to_string(),
        artifact: artifact(),
        job_payload: coordinated_payload("stream-1", &["ai/temper"]),
    })
    .unwrap();
    core.coordinator_mut()
        .register(&register("worker-a", "engineer", "ai/temper", 1));
    core.coordinator_mut()
        .register(&register("worker-b", "engineer", "ai/temper", 1));

    let heartbeat = |worker: &str, jobs: &[&str]| Heartbeat {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker.to_string(),
        jobs: jobs
            .iter()
            .map(|job_id| JobHeartbeat {
                job_id: (*job_id).to_string(),
                attempt_id: Some("attempt-old".to_string()),
                state: HeartbeatState::Running,
                message: format!("report:{job_id}"),
                liveness: None,
            })
            .collect(),
        free_capacity: Some(0),
        worker_pool: None,
        max_concurrent_jobs: None,
        capabilities: Vec::new(),
    };

    let (_, mismatch) = core
        .handle_authenticated_heartbeat(heartbeat("worker-b", &["job-old"]), None)
        .unwrap();
    assert!(mismatch.matched_reports.is_empty());
    assert_eq!(mismatch.rejected_reports[0].job_id, "job-old");
    assert_eq!(
        mismatch.rejected_reports[0].attempt_id.as_deref(),
        Some("attempt-old")
    );
    assert_eq!(mismatch.rejected_reports[0].message, "report:job-old");
    assert_eq!(core.staged_recovery_len(), 1);

    let (_, matched) = core
        .handle_authenticated_heartbeat(heartbeat("worker-a", &["unknown", "job-old"]), None)
        .unwrap();
    assert_eq!(
        matched
            .matched_reports
            .iter()
            .map(|report| report.job_id.as_str())
            .collect::<Vec<_>>(),
        ["job-old"]
    );
    assert_eq!(
        matched
            .rejected_reports
            .iter()
            .map(|report| report.job_id.as_str())
            .collect::<Vec<_>>(),
        ["unknown"]
    );
    assert_eq!(core.staged_recovery_len(), 0);
    assert_eq!(matched.matched_reports[0].message, "report:job-old");
    assert_eq!(matched.rejected_reports[0].message, "report:unknown");
    assert_eq!(
        core.coordinator().assigned_worker("job-old"),
        Some("worker-a")
    );

    let (_, repeated) = core
        .handle_authenticated_heartbeat(heartbeat("worker-a", &["job-old"]), None)
        .unwrap();
    assert_eq!(repeated.matched_reports[0].job_id, "job-old");
    assert_eq!(
        core.coordinator().registry().free_capacity("worker-a"),
        Some(0)
    );
}

#[test]
fn repeated_unknown_heartbeats_retain_the_exact_rejected_report() {
    let mut core = DaemonCore::new();
    core.coordinator_mut()
        .register(&register("worker-a", "engineer", "ai/temper", 1));
    let report = JobHeartbeat {
        job_id: "unknown-job".to_string(),
        attempt_id: Some("attempt-unknown".to_string()),
        state: HeartbeatState::Running,
        message: "still cleaning up".to_string(),
        liveness: None,
    };
    let heartbeat = || Heartbeat {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: "worker-a".to_string(),
        jobs: vec![report.clone()],
        free_capacity: Some(0),
        worker_pool: None,
        max_concurrent_jobs: None,
        capabilities: Vec::new(),
    };

    for _ in 0..2 {
        let (_, recovery) = core
            .handle_authenticated_heartbeat(heartbeat(), None)
            .expect("registered heartbeat");
        assert!(recovery.matched_reports.is_empty());
        assert_eq!(recovery.rejected_reports, [report.clone()]);
    }
}

#[test]
fn stale_heartbeat_attempt_does_not_mutate_the_newer_assignment() {
    let mut core = DaemonCore::new();
    core.coordinator_mut()
        .register(&register("worker-a", "engineer", "ai/temper", 1));
    core.enqueue_job("job-1", "engineer", "ai/temper", artifact(), json!({}));
    let assignment = match core.handle(poll("worker-a")) {
        Some(WorkerProtocolMessage::Assign(assign)) => assign,
        other => panic!("expected assignment, got {other:?}"),
    };
    let stale = JobHeartbeat {
        job_id: "job-1".to_string(),
        attempt_id: Some("attempt-older".to_string()),
        state: HeartbeatState::Running,
        message: "old process".to_string(),
        liveness: None,
    };
    let heartbeat = |report| Heartbeat {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: "worker-a".to_string(),
        jobs: vec![report],
        free_capacity: Some(0),
        worker_pool: None,
        max_concurrent_jobs: None,
        capabilities: Vec::new(),
    };

    let (_, rejected) = core
        .handle_authenticated_heartbeat(heartbeat(stale.clone()), None)
        .unwrap();
    assert_eq!(rejected.rejected_reports, [stale]);
    assert_eq!(
        core.current_assignment_identity("job-1"),
        Some(("worker-a".to_string(), assignment.attempt_id.clone()))
    );

    let current = JobHeartbeat {
        job_id: "job-1".to_string(),
        attempt_id: assignment.attempt_id,
        state: HeartbeatState::Running,
        message: "current process".to_string(),
        liveness: None,
    };
    let (_, matched) = core
        .handle_authenticated_heartbeat(heartbeat(current.clone()), None)
        .unwrap();
    assert_eq!(matched.matched_reports, [current]);
}

#[test]
fn context_authorization_requires_the_exact_current_attempt() {
    let mut core = DaemonCore::new();
    core.coordinator_mut()
        .register(&register("worker-a", "engineer", "ai/temper", 1));
    core.enqueue_job("job-1", "engineer", "ai/temper", artifact(), json!({}));
    let assignment = match core.handle(poll("worker-a")) {
        Some(WorkerProtocolMessage::Assign(assign)) => assign,
        other => panic!("expected assignment, got {other:?}"),
    };
    let attempt_id = assignment.attempt_id.as_deref();

    assert!(
        core.authorize_context_read("worker-a", "job-1", attempt_id, None)
            .unwrap()
            .is_some()
    );
    assert!(
        core.authorize_context_read("worker-a", "job-1", Some("attempt-old"), None)
            .unwrap()
            .is_none()
    );
    assert!(
        core.authorize_context_read("worker-b", "job-1", attempt_id, None)
            .unwrap()
            .is_none()
    );
}
