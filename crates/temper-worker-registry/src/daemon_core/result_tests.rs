// SPDX-License-Identifier: MPL-2.0

use serde_json::json;
use temper_protocol_worker::{ErrorCode, Release, ReleaseDisposition, WorkerProtocolMessage};

use crate::daemon_core::{DaemonCore, RecoveredJob};
use crate::test_support::{artifact, assert_error, poll, register, result_with_attempt};

#[test]
fn result_returns_release_accepted_and_frees_capacity() {
    let mut core = DaemonCore::new();
    core.coordinator_mut()
        .register(&register("worker-a", "engineer", "ai/temper", 1));
    core.enqueue_job("job-1", "engineer", "ai/temper", artifact(), json!({"n":1}));
    core.enqueue_job("job-2", "engineer", "ai/temper", artifact(), json!({"n":2}));

    let assignment = match core.handle(poll("worker-a")) {
        Some(WorkerProtocolMessage::Assign(assign)) => {
            assert_eq!(assign.job_id, "job-1");
            assign
        }
        other => panic!("expected first assign, got {other:?}"),
    };

    match core.handle(result_with_attempt(
        "worker-a",
        "job-1",
        assignment.attempt_id,
    )) {
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
fn result_attempt_fence_rejects_unfenced_and_supersedes_stale_delivery() {
    let mut core = DaemonCore::new();
    core.coordinator_mut()
        .register(&register("worker-a", "engineer", "ai/temper", 1));
    core.enqueue_job("job-1", "engineer", "ai/temper", artifact(), json!({}));
    let assignment = match core.handle(poll("worker-a")) {
        Some(WorkerProtocolMessage::Assign(assign)) => assign,
        other => panic!("expected assignment, got {other:?}"),
    };

    assert_error(
        core.handle(result_with_attempt("worker-a", "job-1", None)),
        ErrorCode::MalformedMessage,
        "unfenced result cannot complete a fenced assignment",
    );
    match core.handle(result_with_attempt(
        "worker-a",
        "job-1",
        Some("older-attempt".to_string()),
    )) {
        Some(WorkerProtocolMessage::Release(release)) => {
            assert_eq!(release.disposition, ReleaseDisposition::Superseded);
            assert_eq!(release.attempt_id.as_deref(), Some("older-attempt"));
        }
        other => panic!("expected superseded release, got {other:?}"),
    }
    assert!(core.in_flight_job("job-1").is_some());

    assert!(matches!(
        core.handle(result_with_attempt(
            "worker-a",
            "job-1",
            assignment.attempt_id,
        )),
        Some(WorkerProtocolMessage::Release(Release {
            disposition: ReleaseDisposition::Accepted,
            ..
        }))
    ));
}

#[test]
fn older_attempt_from_reclaimed_worker_is_superseded_without_mutating_new_assignment() {
    let mut core = DaemonCore::new();
    core.coordinator_mut()
        .register(&register("worker-a", "engineer", "ai/temper", 1));
    core.enqueue_job("job-1", "engineer", "ai/temper", artifact(), json!({}));
    let old_assignment = match core.handle(poll("worker-a")) {
        Some(WorkerProtocolMessage::Assign(assign)) => assign,
        other => panic!("expected old assignment, got {other:?}"),
    };

    assert_eq!(
        core.coordinator_mut().reclaim_worker("worker-a"),
        vec!["job-1".to_string()]
    );
    core.coordinator_mut()
        .register(&register("worker-b", "engineer", "ai/temper", 1));
    let new_assignment = match core.handle(poll("worker-b")) {
        Some(WorkerProtocolMessage::Assign(assign)) => assign,
        other => panic!("expected reassignment, got {other:?}"),
    };
    assert_ne!(old_assignment.attempt_id, new_assignment.attempt_id);

    match core.handle(result_with_attempt(
        "worker-a",
        "job-1",
        old_assignment.attempt_id.clone(),
    )) {
        Some(WorkerProtocolMessage::Release(release)) => {
            assert_eq!(release.disposition, ReleaseDisposition::Superseded);
            assert_eq!(release.worker_id, "worker-a");
            assert_eq!(release.attempt_id, old_assignment.attempt_id);
        }
        other => panic!("expected superseded release, got {other:?}"),
    }
    let current = core
        .in_flight_job("job-1")
        .expect("new attempt remains current");
    assert_eq!(current.attempt_id, new_assignment.attempt_id);
    assert_eq!(
        core.coordinator().assigned_worker("job-1"),
        Some("worker-b")
    );
}

#[test]
fn matching_result_completes_staged_startup_assignment() {
    let mut core = DaemonCore::new();
    core.stage_recovered_job(RecoveredJob {
        job_id: "job-old".to_string(),
        attempt_id: Some("attempt-old".to_string()),
        worker_id: "worker-a".to_string(),
        role: "engineer".to_string(),
        repo: "ai/temper".to_string(),
        artifact: artifact(),
        job_payload: json!({}),
    })
    .unwrap();
    core.coordinator_mut()
        .register(&register("worker-a", "engineer", "ai/temper", 1));

    match core.handle(result_with_attempt(
        "worker-a",
        "job-old",
        Some("attempt-old".to_string()),
    )) {
        Some(WorkerProtocolMessage::Release(release)) => {
            assert_eq!(release.disposition, ReleaseDisposition::Accepted);
            assert_eq!(release.attempt_id.as_deref(), Some("attempt-old"));
        }
        other => panic!("expected accepted staged release, got {other:?}"),
    }
    assert_eq!(core.staged_recovery_len(), 0);
    assert!(core.in_flight_job("job-old").is_none());
}
