//! Worker ↔ daemon transport e2e against the **real** Temper daemon.
//!
//! Both the daemon and the worker run on one skein runtime and communicate via
//! the reusable in-process carrier — the exact path the unified single-process
//! worker uses in production. The [`DaemonHarness`] taps the posted worker
//! result so the test can observe what the real daemon received. No fake daemon,
//! no tokio/axum.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use serde_json::json;
use temper_protocol_worker::{
    Artifact, Assign, Branch, FailureClass, JobResult, RepoOutcome, ResultStatus,
    WORKER_PROTOCOL_VERSION, WorkerAuth,
};
use temper_worker::config::CapabilitySpec;
use temper_worker::{
    ExecutorSelection, JobExecutor, StubExecutor, WorkerConfig, run_worker_with_transport,
};

#[path = "support/real_daemon.rs"]
mod real_daemon;
use real_daemon::DaemonHarness;

fn worker_config() -> WorkerConfig {
    worker_config_with_capacity(1)
}

fn worker_config_with_capacity(max_concurrent_jobs: u32) -> WorkerConfig {
    WorkerConfig {
        daemon_url: "in-process".to_string(),
        worker_id: "worker-1".to_string(),
        worker_pool: None,
        worker_auth: None,
        capabilities: vec![CapabilitySpec {
            repo: "ai/smith".to_string(),
            role: "engineer".to_string(),
        }],
        role_identities: std::collections::BTreeMap::new(),
        max_concurrent_jobs,
        poll_wait: Duration::from_millis(25),
        heartbeat_interval: Duration::from_millis(25),
        liveness_limits: Default::default(),
        result_root: std::env::temp_dir().join(format!(
            "temper-worker-test-results-{}",
            uuid::Uuid::new_v4()
        )),
        agent_traces: Default::default(),
        executor: ExecutorSelection::Stub,
    }
}

fn assign_for(config: &WorkerConfig) -> Assign {
    assign_for_job(config, "job-123")
}

fn assign_for_job(config: &WorkerConfig, job_id: &str) -> Assign {
    Assign {
        protocol_version: WORKER_PROTOCOL_VERSION,
        trace_context: None,
        job_id: job_id.to_string(),
        attempt_id: Some(format!("attempt-{job_id}")),
        role: config.capabilities[0].role.clone(),
        repo: config.capabilities[0].repo.clone(),
        artifact: Artifact {
            item: json!(78),
            kind: "intake".to_string(),
        },
        job_payload: json!({}),
    }
}

#[derive(Clone)]
struct BlockingExecutor {
    probe: Arc<ConcurrencyProbe>,
}

impl JobExecutor for BlockingExecutor {
    fn execute(
        &self,
        assign: Assign,
        _context: temper_worker::JobExecutionContext,
    ) -> impl Future<Output = temper_worker::JobOutcome> + Send {
        let probe = Arc::clone(&self.probe);
        async move {
            probe.job_started(assign.job_id.clone());
            WaitForRelease {
                probe: Arc::clone(&probe),
            }
            .await;
            probe.job_finished();
            temper_worker::JobOutcome::Success {
                repos: vec![RepoOutcome {
                    repo: assign.repo,
                    branch: Branch {
                        name: format!("temper-worker/concurrency/{}", assign.job_id),
                        head_sha: "0000000000000000000000000000000000000000".to_string(),
                    },
                }],
                title: None,
                body: None,
                summary: Some("blocking executor released".to_string()),
                details: None,
            }
        }
    }
}

struct ConcurrencyProbe {
    state: Mutex<ProbeState>,
    started_tx: temper_engine_io::CqSender<String>,
}

#[derive(Default)]
struct ProbeState {
    current: u32,
    max: u32,
    released: bool,
    wakers: Vec<Waker>,
}

impl ConcurrencyProbe {
    fn new(started_tx: temper_engine_io::CqSender<String>) -> Self {
        Self {
            state: Mutex::new(ProbeState::default()),
            started_tx,
        }
    }

    fn job_started(&self, job_id: String) {
        {
            let mut state = self.state.lock().expect("probe lock");
            state.current += 1;
            state.max = state.max.max(state.current);
        }
        let _ = self.started_tx.send(job_id);
    }

    fn job_finished(&self) {
        let mut state = self.state.lock().expect("probe lock");
        state.current -= 1;
    }

    fn max_in_flight(&self) -> u32 {
        self.state.lock().expect("probe lock").max
    }

    fn release_all(&self) {
        let wakers = {
            let mut state = self.state.lock().expect("probe lock");
            state.released = true;
            std::mem::take(&mut state.wakers)
        };
        for waker in wakers {
            waker.wake();
        }
    }
}

struct WaitForRelease {
    probe: Arc<ConcurrencyProbe>,
}

impl Future for WaitForRelease {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self.probe.state.lock().expect("probe lock");
        if state.released {
            Poll::Ready(())
        } else {
            state.wakers.push(cx.waker().clone());
            Poll::Pending
        }
    }
}

async fn recv_started(
    cx: &skein::cx::Cx,
    started_rx: &mut temper_engine_io::CqReceiver<String>,
) -> String {
    match skein::time::timeout(
        temper_engine_io::runtime::timer_now(cx),
        Duration::from_secs(2),
        Box::pin(started_rx.recv()),
    )
    .await
    {
        Ok(Some(job_id)) => job_id,
        Ok(None) => panic!("started channel closed before both jobs started"),
        Err(_) => panic!("timed out waiting for both jobs to start"),
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
fn daemon_transport_success_stub_registers_polls_runs_and_posts_result() {
    let result = run_until_result(StubExecutor::success().into());

    assert_eq!(result.job_id, "job-123");
    assert_eq!(result.status, ResultStatus::Success);
    assert_eq!(result.repos.len(), 1);
    assert_eq!(result.failure, None);
}

#[test]
fn daemon_transport_in_process_worker_carries_pool_auth_metadata() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let mut auth = temper_engine::WorkerPoolAuthConfig::new();
        auth.insert_pool("builders", Some(WorkerAuth::bearer("builders-secret")));
        let mut harness = DaemonHarness::start_with_worker_auth(&handle, auth);
        let mut config = worker_config();
        config.worker_pool = Some("builders".to_string());
        config.worker_auth = Some(WorkerAuth::bearer("builders-secret"));
        harness.enqueue(&assign_for(&config)).await;

        let transport = harness.transport();
        let worker_handle = handle.clone();
        handle.spawn(async move {
            let _ = run_worker_with_transport(
                worker_handle,
                config,
                StubExecutor::success().into(),
                transport,
            )
            .await;
        });

        let result = harness.await_result().await;
        assert_eq!(result.job_id, "job-123");
        assert_eq!(result.status, ResultStatus::Success);
    });
}

#[test]
fn daemon_transport_standalone_in_process_worker_runs_two_engineer_jobs_concurrently() {
    temper_engine_io::block_on_with(move |cx, handle| async move {
        let mut harness = DaemonHarness::start(&handle);
        let config = worker_config_with_capacity(2);
        harness.enqueue(&assign_for_job(&config, "job-a")).await;
        harness.enqueue(&assign_for_job(&config, "job-b")).await;

        let (started_tx, mut started_rx) = temper_engine_io::channel();
        let probe = Arc::new(ConcurrencyProbe::new(started_tx));
        let executor = Arc::new(BlockingExecutor {
            probe: Arc::clone(&probe),
        });
        let transport = harness.transport();
        let worker_handle = handle.clone();
        handle.spawn(async move {
            let _ = run_worker_with_transport(worker_handle, config, executor, transport).await;
        });

        let first = recv_started(&cx, &mut started_rx).await;
        let second = recv_started(&cx, &mut started_rx).await;
        assert_ne!(first, second, "two distinct jobs should start");
        assert_eq!(
            probe.max_in_flight(),
            2,
            "both engineer jobs should be in flight before either is released"
        );

        probe.release_all();
        let first_result = harness.await_result().await;
        let second_result = harness.await_result().await;
        let mut completed = [first_result.job_id, second_result.job_id];
        completed.sort();
        assert_eq!(completed, ["job-a", "job-b"]);
    });
}

#[test]
fn daemon_transport_failure_stub_registers_polls_runs_and_posts_failure_result() {
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
