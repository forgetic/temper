//! Worker ↔ daemon transport e2e against the **real** Temper daemon.
//!
//! Both the daemon and the worker run on one skein runtime and communicate via
//! the daemon's in-process carrier — the exact path the unified single-process
//! worker uses in production. The [`DaemonHarness`] instruments the real daemon
//! with a recording applier so the test can observe the result the worker
//! posted. No fake daemon, no tokio/axum.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use temper_worker::config::CapabilitySpec;
use temper_worker::{
    ExecutorSelection, JobExecutor, StubExecutor, WorkerConfig, run_worker_with_transport,
};
use temper_worker_protocol::{
    Artifact, Assign, FailureClass, JobResult, ResultStatus, WORKER_PROTOCOL_VERSION,
};

#[path = "support/real_daemon.rs"]
mod real_daemon;
use real_daemon::DaemonHarness;

fn worker_config() -> WorkerConfig {
    WorkerConfig {
        daemon_url: "in-process".to_string(),
        worker_id: "worker-1".to_string(),
        capabilities: vec![CapabilitySpec {
            repo: "ai/smith".to_string(),
            role: "engineer".to_string(),
        }],
        role_identities: std::collections::BTreeMap::new(),
        max_concurrent_jobs: 1,
        poll_wait: Duration::from_millis(25),
        heartbeat_interval: Duration::from_millis(25),
        executor: ExecutorSelection::Stub,
    }
}

fn assign_for(config: &WorkerConfig) -> Assign {
    Assign {
        protocol_version: WORKER_PROTOCOL_VERSION,
        job_id: "job-123".to_string(),
        role: config.capabilities[0].role.clone(),
        repo: config.capabilities[0].repo.clone(),
        artifact: Artifact {
            item: json!(78),
            kind: "intake".to_string(),
        },
        job_payload: json!({}),
    }
}

/// Stand up the real daemon, enqueue one job matching the worker's capability,
/// run a real worker on the same runtime through the daemon's in-process
/// carrier, and return the result the daemon applied. The worker loops forever;
/// it is spawned detached and the test returns once the result is applied.
fn run_until_result<E>(executor: Arc<E>) -> JobResult
where
    E: JobExecutor + Send + Sync + 'static,
{
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let mut harness = DaemonHarness::start(&handle);
        let config = worker_config();
        harness.enqueue(&assign_for(&config)).await;

        let transport = harness.transport();
        let worker_handle = handle.clone();
        handle.spawn(async move {
            let _ = run_worker_with_transport(worker_handle, config, executor, transport).await;
        });

        harness.await_result().await
    })
}

#[test]
fn success_stub_registers_polls_runs_and_posts_result() {
    let result = run_until_result(StubExecutor::success().into());

    assert_eq!(result.job_id, "job-123");
    assert_eq!(result.status, ResultStatus::Success);
    assert_eq!(result.repos.len(), 1);
    assert_eq!(result.failure, None);
}

#[test]
fn failure_stub_registers_polls_runs_and_posts_failure_result() {
    let result = run_until_result(
        StubExecutor::failure(FailureClass::Permanent, "configured failure").into(),
    );

    assert_eq!(result.job_id, "job-123");
    assert_eq!(result.status, ResultStatus::Failure);
    assert!(result.repos.is_empty());
    let failure = result.failure.expect("failure details present");
    assert_eq!(failure.class, FailureClass::Permanent);
    assert_eq!(failure.message, "configured failure");
}
