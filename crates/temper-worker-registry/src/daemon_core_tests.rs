// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeSet;

use serde_json::json;
use temper_protocol_worker::{
    Assign, ErrorCode, Release, ReleaseDisposition, WORKER_PROTOCOL_VERSION, WorkerAuth,
    WorkerProtocolMessage,
};

use crate::daemon_core::{DaemonCore, InFlightJob, WorkerAuthError};
use crate::test_support::{
    artifact, assert_error, coordinated_payload, heartbeat, poll, register, register_multi, result,
};
use crate::{WorkerPoolAuthConfig, WorkerPoolPolicy};

fn pool_auth_config() -> WorkerPoolAuthConfig {
    let mut config = WorkerPoolAuthConfig::new();
    config.insert_pool("builders", Some(WorkerAuth::bearer("builders-secret")));
    config.insert_pool("reviewers", Some(WorkerAuth::bearer("reviewers-secret")));
    config
}

fn auth(token: &str) -> WorkerAuth {
    WorkerAuth::bearer(token.to_string())
}

fn pool_register(worker_id: &str, pool: &str, role: &str, repo: &str) -> WorkerProtocolMessage {
    let mut register = register(worker_id, role, repo, 1);
    register.worker_pool = Some(pool.to_string());
    WorkerProtocolMessage::Register(register)
}

fn assert_auth_error(
    result: Result<Option<WorkerProtocolMessage>, WorkerAuthError>,
    message: &str,
) {
    match result {
        Err(error) => assert_eq!(error.message(), message),
        Ok(other) => panic!("expected auth error, got {other:?}"),
    }
}

fn builders_policy() -> Vec<WorkerPoolPolicy> {
    vec![WorkerPoolPolicy::new(
        "builders",
        vec!["engineer".to_string()],
        vec!["ai/temper".to_string(), "ai/smith".to_string()],
        Some(2),
    )]
}

#[test]
fn worker_pool_auth_rejects_missing_wrong_and_cross_pool_register_tokens() {
    let mut core = DaemonCore::with_pool_policies(builders_policy());
    core.configure_worker_pool_auth(pool_auth_config());

    assert_auth_error(
        core.handle_authenticated(
            pool_register("worker-a", "builders", "engineer", "ai/temper"),
            None,
        ),
        "worker pool `builders` requires worker_token authentication",
    );
    assert_auth_error(
        core.handle_authenticated(
            pool_register("worker-a", "builders", "engineer", "ai/temper"),
            Some(&auth("wrong-secret")),
        ),
        "worker pool `builders` worker_token authentication failed",
    );
    assert_auth_error(
        core.handle_authenticated(
            pool_register("worker-a", "builders", "engineer", "ai/temper"),
            Some(&auth("reviewers-secret")),
        ),
        "worker pool `builders` worker_token authentication failed",
    );

    assert_eq!(
        core.handle_authenticated(
            pool_register("worker-a", "builders", "engineer", "ai/temper"),
            Some(&auth("builders-secret")),
        )
        .expect("valid pool token authenticates"),
        None
    );
}

#[test]
fn worker_pool_auth_rechecks_registered_pool_for_poll_result_and_heartbeat() {
    let mut core = DaemonCore::with_pool_policies(builders_policy());
    core.configure_worker_pool_auth(pool_auth_config());
    assert_eq!(
        core.handle_authenticated(
            pool_register("worker-a", "builders", "engineer", "ai/temper"),
            Some(&auth("builders-secret")),
        )
        .expect("register authenticates"),
        None
    );
    core.enqueue_job("job-1", "engineer", "ai/temper", artifact(), json!({}));

    assert_auth_error(
        core.handle_authenticated(poll("worker-a"), None),
        "worker pool `builders` requires worker_token authentication",
    );
    assert_auth_error(
        core.handle_authenticated(poll("worker-a"), Some(&auth("reviewers-secret"))),
        "worker pool `builders` worker_token authentication failed",
    );

    match core
        .handle_authenticated(poll("worker-a"), Some(&auth("builders-secret")))
        .expect("poll authenticates")
    {
        Some(WorkerProtocolMessage::Assign(assign)) => assert_eq!(assign.job_id, "job-1"),
        other => panic!("expected assignment, got {other:?}"),
    }

    assert_auth_error(
        core.handle_authenticated(result("worker-a", "job-1"), Some(&auth("reviewers-secret"))),
        "worker pool `builders` worker_token authentication failed",
    );
    assert!(
        core.in_flight_job("job-1").is_some(),
        "unauthorized result must not complete the job"
    );

    let mut mismatched_heartbeat = match heartbeat("worker-a") {
        WorkerProtocolMessage::Heartbeat(heartbeat) => heartbeat,
        _ => unreachable!(),
    };
    mismatched_heartbeat.worker_pool = Some("reviewers".to_string());
    assert_auth_error(
        core.handle_authenticated(
            WorkerProtocolMessage::Heartbeat(mismatched_heartbeat),
            Some(&auth("builders-secret")),
        ),
        "worker `worker-a` sent worker_pool `reviewers` but registered to `builders`",
    );

    match core
        .handle_authenticated(result("worker-a", "job-1"), Some(&auth("builders-secret")))
        .expect("result authenticates")
    {
        Some(WorkerProtocolMessage::Release(release)) => assert_eq!(release.job_id, "job-1"),
        other => panic!("expected release, got {other:?}"),
    }
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
