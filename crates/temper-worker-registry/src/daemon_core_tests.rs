// SPDX-License-Identifier: MPL-2.0

use std::collections::{BTreeMap, BTreeSet};

use serde_json::json;
use temper_protocol_worker::{
    Assign, AttemptCancellation, CancelAttempts, ErrorCode, Heartbeat, HeartbeatState,
    JobHeartbeat, Release, ReleaseDisposition, WORKER_PROTOCOL_VERSION, WorkerProtocolMessage,
};

use crate::WorkerPoolPolicy;
use crate::daemon_core::{DaemonCore, InFlightJob, RecoveredJob};
use crate::test_support::{
    artifact, assert_error, coordinated_payload, heartbeat, poll, register, register_multi,
    result_with_attempt,
};

fn builders_policy() -> Vec<WorkerPoolPolicy> {
    vec![WorkerPoolPolicy::new(
        "builders",
        vec!["engineer".to_string()],
        vec!["ai/temper".to_string(), "ai/smith".to_string()],
        Some(2),
    )]
}

#[test]
fn register_validates_selected_pool_policy_before_dispatch() {
    let mut core = DaemonCore::with_pool_policies(builders_policy());
    let mut msg = register_multi("worker-a", "engineer", &["ai/temper", "ai/smith"], 2);
    msg.worker_pool = Some("builders".to_string());

    assert_eq!(core.handle(WorkerProtocolMessage::Register(msg)), None);
    core.enqueue_job(
        "job-1",
        "engineer",
        "ai/temper",
        artifact(),
        coordinated_payload("coord-1", &["ai/temper", "ai/smith"]),
    );

    match core.handle(poll("worker-a")) {
        Some(WorkerProtocolMessage::Assign(assign)) => assert_eq!(assign.job_id, "job-1"),
        other => panic!("expected assign, got {other:?}"),
    }
}

#[test]
fn register_rejects_selected_pool_that_violates_policy() {
    let mut core = DaemonCore::with_pool_policies(builders_policy());
    let mut too_large = register("worker-a", "engineer", "ai/temper", 3);
    too_large.worker_pool = Some("builders".to_string());

    assert_error(
        core.handle(WorkerProtocolMessage::Register(too_large)),
        ErrorCode::RegistrationRejected,
        "worker capacity 3 exceeds worker pool `builders` max_concurrent_jobs 2",
    );
    assert_error(
        core.handle(poll("worker-a")),
        ErrorCode::UnknownWorker,
        "unknown worker",
    );

    let mut wrong_repo = register("worker-a", "engineer", "ai/other", 1);
    wrong_repo.worker_pool = Some("builders".to_string());
    assert_error(
        core.handle(WorkerProtocolMessage::Register(wrong_repo)),
        ErrorCode::RegistrationRejected,
        "worker capability `ai/other:engineer` is outside worker pool `builders` policy",
    );
}

#[test]
fn register_without_pool_preserves_legacy_capabilities_with_pool_policies() {
    let mut core = DaemonCore::with_pool_policies(builders_policy());
    assert_eq!(
        core.handle(WorkerProtocolMessage::Register(register(
            "legacy-worker",
            "legacy",
            "legacy/repo",
            1,
        ))),
        None
    );
    core.enqueue_job("job-1", "legacy", "legacy/repo", artifact(), json!({}));

    match core.handle(poll("legacy-worker")) {
        Some(WorkerProtocolMessage::Assign(assign)) => assert_eq!(assign.job_id, "job-1"),
        other => panic!("expected legacy assign, got {other:?}"),
    }
}

#[test]
fn register_rejects_empty_worker_id() {
    let mut core = DaemonCore::new();

    assert_error(
        core.handle(WorkerProtocolMessage::Register(register(
            " ",
            "engineer",
            "ai/temper",
            1,
        ))),
        ErrorCode::RegistrationRejected,
        "worker_id must not be empty",
    );
}

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
}

#[test]
fn engineer_source_issue_and_pr_repair_share_one_workstream_slot() {
    let mut core = DaemonCore::new();
    core.coordinator_mut()
        .register(&register("engineer-1", "engineer", "ai/temper", 3));

    core.enqueue_job(
        "ai/temper/issue-463/engineer/code_ready",
        "engineer",
        "ai/temper",
        artifact(),
        coordinated_payload("pr-for-code-463", &["ai/temper"]),
    );
    core.enqueue_job(
        "ai/temper/pull_request-464/engineer/pr_ci_failed",
        "engineer",
        "ai/temper",
        artifact(),
        coordinated_payload("pr-for-code-463", &["ai/temper"]),
    );
    core.enqueue_job(
        "ai/temper/issue-999/engineer/code_ready",
        "engineer",
        "ai/temper",
        artifact(),
        coordinated_payload("pr-for-code-999", &["ai/temper"]),
    );

    match core.handle(poll("engineer-1")) {
        Some(WorkerProtocolMessage::Assign(assign)) => {
            assert_eq!(assign.job_id, "ai/temper/issue-463/engineer/code_ready")
        }
        other => panic!("expected source issue assign, got {other:?}"),
    }
    // Capacity is still available, but the PR repair job shares the issue job's
    // engineer workstream key, so the unrelated engineer job gets the next slot.
    match core.handle(poll("engineer-1")) {
        Some(WorkerProtocolMessage::Assign(assign)) => {
            assert_eq!(assign.job_id, "ai/temper/issue-999/engineer/code_ready")
        }
        other => panic!("expected unrelated assign, got {other:?}"),
    }

    assert_eq!(core.in_flight_role_count("engineer"), 2);
    assert_eq!(
        core.in_flight_job("ai/temper/pull_request-464/engineer/pr_ci_failed"),
        None
    );
    assert_eq!(
        core.queued_jobs()
            .iter()
            .map(|job| job.job_id.as_str())
            .collect::<Vec<_>>(),
        vec!["ai/temper/pull_request-464/engineer/pr_ci_failed"]
    );
    assert_error(
        core.handle(poll("engineer-1")),
        ErrorCode::PollTimeout,
        "no work available",
    );
}

#[test]
fn active_workstream_covers_pending_and_assigned_jobs() {
    let mut core = DaemonCore::new();
    core.coordinator_mut()
        .register(&register("worker-a", "engineer", "ai/temper", 1));
    core.enqueue_job(
        "assigned",
        "engineer",
        "ai/temper",
        artifact(),
        coordinated_payload("pr-for-code-7", &["ai/temper"]),
    );
    core.enqueue_job(
        "pending",
        "engineer",
        "ai/temper",
        artifact(),
        coordinated_payload("pr-for-code-8", &["ai/temper"]),
    );

    match core.handle(poll("worker-a")) {
        Some(WorkerProtocolMessage::Assign(assign)) => assert_eq!(assign.job_id, "assigned"),
        other => panic!("expected assign, got {other:?}"),
    }

    assert!(core.workstream_active_by_correlation_key("pr-for-code-7"));
    assert!(core.workstream_active_by_correlation_key(" pr-for-code-8 "));
    assert!(!core.workstream_active_by_correlation_key("pr-for-code-9"));
    assert!(!core.workstream_active_by_correlation_key(" "));
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
            attempt_id: core.in_flight_job("job-1").unwrap().attempt_id,
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
fn scoped_pending_reconcile_removes_job_context_only_for_pruned_pending_jobs() {
    let mut core = DaemonCore::new();
    core.coordinator_mut()
        .register(&register("worker-a", "engineer", "ai/temper", 1));
    core.enqueue_job(
        "assigned",
        "engineer",
        "ai/temper",
        artifact(),
        json!({"n":0}),
    );
    match core.handle(poll("worker-a")) {
        Some(WorkerProtocolMessage::Assign(assign)) => assert_eq!(assign.job_id, "assigned"),
        other => panic!("expected assign, got {other:?}"),
    }

    core.enqueue_job("stale", "engineer", "ai/temper", artifact(), json!({"n":1}));
    core.enqueue_job(
        "current",
        "engineer",
        "ai/temper",
        artifact(),
        json!({"n":2}),
    );
    core.enqueue_job(
        "other-role",
        "architect",
        "ai/temper",
        artifact(),
        json!({"n":3}),
    );
    core.enqueue_job(
        "other-repo",
        "engineer",
        "ai/other",
        artifact(),
        json!({"n":4}),
    );

    let current = BTreeSet::from(["current".to_string()]);
    assert_eq!(
        core.retain_pending_jobs_for_scope("ai/temper", "engineer", &current),
        vec!["stale".to_string()]
    );

    assert_eq!(
        core.queued_jobs()
            .iter()
            .map(|job| job.job_id.as_str())
            .collect::<Vec<_>>(),
        vec!["current", "other-role", "other-repo"]
    );
    assert!(core.in_flight_job("assigned").is_some());
    assert_eq!(core.in_flight_job("stale"), None);
}

#[test]
fn targeted_pending_reconcile_prunes_only_the_exact_artifact() {
    let mut core = DaemonCore::new();
    let mut selected = artifact();
    selected.item = json!(7);
    let mut unrelated = artifact();
    unrelated.item = json!(8);
    core.enqueue_job(
        "selected-stale",
        "engineer",
        "ai/temper",
        selected.clone(),
        json!({}),
    );
    core.enqueue_job("unrelated", "engineer", "ai/temper", unrelated, json!({}));

    assert_eq!(
        core.retain_pending_jobs_for_artifact(
            "ai/temper",
            "engineer",
            &selected,
            &BTreeSet::new(),
        ),
        vec!["selected-stale".to_string()]
    );
    assert_eq!(
        core.queued_jobs()
            .iter()
            .map(|job| job.job_id.as_str())
            .collect::<Vec<_>>(),
        vec!["unrelated"]
    );
}

#[test]
fn role_saturation_uses_configured_limit_and_preserves_pending_order() {
    let mut core = DaemonCore::with_role_limits(BTreeMap::from([("architect".to_string(), 1)]));
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
    assert!(core.role_saturation("architect").is_none());
    assert_eq!(core.in_flight_role_count("architect"), 0);

    // Claiming the slot for job-a makes the role busy with job-b queued behind.
    match core.handle(poll("architect-1")) {
        Some(WorkerProtocolMessage::Assign(assign)) => assert_eq!(assign.job_id, "job-a"),
        other => panic!("expected assign, got {other:?}"),
    }
    assert_eq!(core.in_flight_role_count("architect"), 1);
    let saturation = core
        .role_saturation("architect")
        .expect("configured role is saturated");
    assert_eq!(saturation.concurrency, 1);
    assert_eq!(saturation.pending.len(), 1, "only job-b is queued behind");
    assert_eq!(saturation.pending[0].0, "acme/widgets");

    // A role without a configured finite limit is never reported saturated.
    assert!(core.role_saturation("engineer").is_none());
}

#[test]
fn zero_limit_saturates_with_ordered_pending_work_and_no_in_flight_holder() {
    let mut core = DaemonCore::with_role_limits(BTreeMap::from([("engineer".to_string(), 0)]));
    core.coordinator_mut().register(&register_multi(
        "worker",
        "engineer",
        &["acme/api", "acme/widgets"],
        4,
    ));
    core.enqueue_job(
        "job-a",
        "engineer",
        "acme/api",
        temper_protocol_worker::Artifact {
            item: json!(7),
            kind: "issue".to_string(),
        },
        json!({}),
    );
    core.enqueue_job(
        "job-b",
        "engineer",
        "acme/widgets",
        temper_protocol_worker::Artifact {
            item: json!(8),
            kind: "pull_request".to_string(),
        },
        json!({}),
    );

    assert_error(
        core.handle(poll("worker")),
        ErrorCode::PollTimeout,
        "no work available",
    );
    assert_eq!(core.in_flight_role_count("engineer"), 0);
    let saturation = core.role_saturation("engineer").unwrap();
    assert_eq!(saturation.concurrency, 0);
    assert_eq!(
        saturation
            .pending
            .iter()
            .map(|(repo, _artifact)| repo.as_str())
            .collect::<Vec<_>>(),
        vec!["acme/api", "acme/widgets"]
    );
}

#[test]
fn unlimited_role_never_reports_saturation_from_worker_exhaustion() {
    let mut core = DaemonCore::new();
    core.coordinator_mut()
        .register(&register("worker", "engineer", "ai/temper", 1));
    core.enqueue_job("job-a", "engineer", "ai/temper", artifact(), json!({}));
    core.enqueue_job("job-b", "engineer", "ai/temper", artifact(), json!({}));
    assert!(matches!(
        core.handle(poll("worker")),
        Some(WorkerProtocolMessage::Assign(_))
    ));

    assert!(core.role_saturation("engineer").is_none());
}

#[test]
fn combined_constructor_preserves_pool_policies_and_role_limits() {
    let core = DaemonCore::with_pool_policies_and_role_limits(
        builders_policy(),
        BTreeMap::from([("engineer".to_string(), 2)]),
    );

    assert_eq!(core.configured_role_limit("engineer"), Some(2));
    assert_eq!(core.configured_role_limit("architect"), None);
    assert_eq!(
        core.configured_role_limits(),
        &BTreeMap::from([("engineer".to_string(), 2)])
    );
}

#[test]
fn completed_job_is_no_longer_in_flight() {
    let mut core = DaemonCore::new();
    core.coordinator_mut()
        .register(&register("worker-a", "engineer", "ai/temper", 1));
    core.enqueue_job("job-1", "engineer", "ai/temper", artifact(), json!({"k":1}));

    let assignment = match core.handle(poll("worker-a")) {
        Some(WorkerProtocolMessage::Assign(assign)) => {
            assert_eq!(assign.job_id, "job-1");
            assign
        }
        other => panic!("expected assign, got {other:?}"),
    };
    let _ = core.handle(result_with_attempt(
        "worker-a",
        "job-1",
        assignment.attempt_id,
    ));

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
fn matching_heartbeat_reattaches_staged_assignment_and_rejects_other_ids() {
    let mut core = DaemonCore::new();
    core.stage_recovered_job(RecoveredJob {
        job_id: "job-old".to_string(),
        attempt_id: None,
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
                attempt_id: None,
                state: HeartbeatState::Running,
                message: String::new(),
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
    assert!(mismatch.matched_job_ids.is_empty());
    assert_eq!(mismatch.rejected_job_ids, ["job-old"]);
    assert_eq!(core.staged_recovery_len(), 1);

    let (_, matched) = core
        .handle_authenticated_heartbeat(heartbeat("worker-a", &["unknown", "job-old"]), None)
        .unwrap();
    assert_eq!(matched.matched_job_ids, ["job-old"]);
    assert_eq!(matched.rejected_job_ids, ["unknown"]);
    assert_eq!(core.staged_recovery_len(), 0);
    assert_eq!(
        core.coordinator().assigned_worker("job-old"),
        Some("worker-a")
    );

    let (_, repeated) = core
        .handle_authenticated_heartbeat(heartbeat("worker-a", &["job-old"]), None)
        .unwrap();
    assert_eq!(repeated.matched_job_ids, ["job-old"]);
    assert_eq!(
        core.coordinator().registry().free_capacity("worker-a"),
        Some(0)
    );
}

#[test]
fn unreattached_recovery_is_returned_once_for_orphan_convergence() {
    let mut core = DaemonCore::new();
    let recovered = RecoveredJob {
        job_id: "job-old".to_string(),
        attempt_id: None,
        worker_id: "worker-a".to_string(),
        role: "engineer".to_string(),
        repo: "ai/temper".to_string(),
        artifact: artifact(),
        job_payload: json!({}),
    };
    core.stage_recovered_job(recovered.clone()).unwrap();

    assert_eq!(core.take_unreattached_recovered_jobs(), vec![recovered]);
    assert!(core.take_unreattached_recovered_jobs().is_empty());
    assert_eq!(core.job_context("job-old"), None);
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
fn inbound_daemon_messages_are_malformed() {
    let mut core = DaemonCore::new();
    let assign = WorkerProtocolMessage::Assign(Assign {
        protocol_version: WORKER_PROTOCOL_VERSION,
        trace_context: None,
        job_id: "job-1".to_string(),
        attempt_id: Some("attempt-1".to_string()),
        role: "engineer".to_string(),
        repo: "ai/temper".to_string(),
        artifact: artifact(),
        job_payload: json!({}),
    });
    let cancel = WorkerProtocolMessage::CancelAttempts(
        CancelAttempts::new(
            "worker-a",
            vec![
                AttemptCancellation::ownership_lost(
                    "worker-a",
                    "job-1",
                    "attempt-1",
                    "durable assignment was removed",
                )
                .expect("valid cancellation"),
            ],
        )
        .expect("valid directive"),
    );
    let release = WorkerProtocolMessage::Release(Release {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: "worker-a".to_string(),
        job_id: "job-1".to_string(),
        attempt_id: Some("attempt-1".to_string()),
        disposition: ReleaseDisposition::Accepted,
        message: None,
    });

    assert_error(
        core.handle(assign),
        ErrorCode::MalformedMessage,
        "daemon-to-worker message received inbound",
    );
    assert_error(
        core.handle(cancel),
        ErrorCode::MalformedMessage,
        "daemon-to-worker message received inbound",
    );
    assert_error(
        core.handle(release),
        ErrorCode::MalformedMessage,
        "daemon-to-worker message received inbound",
    );
}
