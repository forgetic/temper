// SPDX-License-Identifier: MPL-2.0

use serde_json::json;
use temper_protocol_worker::{WorkerAuth, WorkerProtocolMessage};

use crate::daemon_core::{DaemonCore, WorkerAuthError};
use crate::test_support::{artifact, heartbeat, poll, register, result};
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

fn builders_policy() -> Vec<WorkerPoolPolicy> {
    vec![WorkerPoolPolicy::new(
        "builders",
        vec!["engineer".to_string()],
        vec!["ai/temper".to_string(), "ai/smith".to_string()],
        Some(2),
    )]
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
