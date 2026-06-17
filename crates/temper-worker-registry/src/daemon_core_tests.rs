// SPDX-License-Identifier: MPL-2.0

use serde_json::json;
use temper_protocol_worker::{
    Assign, ErrorCode, Release, ReleaseDisposition, WORKER_PROTOCOL_VERSION, WorkerProtocolMessage,
};

use crate::daemon_core::{DaemonCore, InFlightJob};
use crate::test_support::{
    artifact, assert_error, coordinated_payload, heartbeat, poll, register, register_multi, result,
};

#[test]
fn coordinated_job_dispatches_only_to_an_all_repo_capable_worker() {
    let mut core = DaemonCore::new();
    let payload = coordinated_payload("coord-1", &["ai/temper", "ai/smith", "ai/skein"]);
    core.enqueue_job("job-coord", "engineer", "ai/temper", artifact(), payload);

    core.coordinator_mut().register(&register_multi(
        "partial",
        "engineer",
        &["ai/temper", "ai/smith"],
        1,
    ));
    assert_error(
        core.handle(poll("partial")),
        ErrorCode::PollTimeout,
        "no work available",
    );

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
fn role_saturation_names_same_role_pending_behind_a_busy_holder() {
    let mut core = DaemonCore::new();
    core.coordinator_mut().register(&register_multi(
        "architect-1",
        "architect",
        &["acme/api", "acme/widgets"],
        1,
    ));
    // Two architect jobs across two repos; the single architect slot serializes
    // them, so the second queues behind the first.
    core.enqueue_job("job-a", "architect", "acme/api", artifact(), json!({"k":1}));
    core.enqueue_job(
        "job-b",
        "architect",
        "acme/widgets",
        artifact(),
        json!({"k":2}),
    );

    // Nothing is in flight yet -> the role is not saturated.
    assert!(core.role_saturation("architect").is_empty());
    assert_eq!(core.in_flight_role_count("architect"), 0);

    // Claiming the slot for job-a makes the role busy with job-b queued behind.
    match core.handle(poll("architect-1")) {
        Some(WorkerProtocolMessage::Assign(assign)) => assert_eq!(assign.job_id, "job-a"),
        other => panic!("expected assign, got {other:?}"),
    }
    assert_eq!(core.in_flight_role_count("architect"), 1);
    let waiting = core.role_saturation("architect");
    assert_eq!(waiting.len(), 1, "only job-b is queued behind");
    assert_eq!(waiting[0].0, "acme/widgets");

    // A different idle role is never reported saturated.
    assert!(core.role_saturation("engineer").is_empty());
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
