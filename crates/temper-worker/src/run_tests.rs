use std::collections::BTreeMap;
use std::future::Future;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use skein::cx::Cx;
use temper_protocol_activity::AgentActivityCapturePolicyV1;
use temper_protocol_agent::{
    AgentSessionState, WorkspaceContext, WorkspaceRepository, WorkspaceWorkItem,
};
use temper_protocol_worker::{
    Artifact, Assign, FailureClass, WORKER_PROTOCOL_VERSION, WorkerAuth, WorkerProtocolMessage,
};
use temper_worker_io::{CqSender, channel};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::prelude::*;

use super::*;
use crate::config::{ExecutorSelection, WorkerAgentTraceConfig, WorkerLivenessLimits};
use crate::executor::{JobExecutionContext, JobOutcome};

struct ControlledExecutor {
    started: CqSender<crate::JobCancellation>,
    finished: Arc<AtomicBool>,
    finish_at: crate::JobCancellationRequest,
}

impl JobExecutor for ControlledExecutor {
    fn execute(
        &self,
        _assign: Assign,
        context: JobExecutionContext,
    ) -> impl Future<Output = JobOutcome> + Send {
        let cancellation = context.cancellation;
        let owner = cancellation.register_async_owner();
        let _ = self.started.send(cancellation.clone());
        let finished = Arc::clone(&self.finished);
        let finish_at = self.finish_at;
        async move {
            let mut observed = None;
            loop {
                let request =
                    std::future::poll_fn(|cx| cancellation.poll_request(observed, cx)).await;
                observed = Some(request);
                if request >= finish_at {
                    break;
                }
            }
            finished.store(true, Ordering::Release);
            drop(owner);
            JobOutcome::Failure {
                class: FailureClass::Canceled,
                message: "component stopped".to_string(),
            }
        }
    }
}

struct BlockingExecutor {
    started: CqSender<crate::JobCancellation>,
}

impl JobExecutor for BlockingExecutor {
    fn execute(
        &self,
        _assign: Assign,
        context: JobExecutionContext,
    ) -> impl Future<Output = JobOutcome> + Send {
        let cancellation = context.cancellation;
        let owner = cancellation.register_async_owner();
        let _ = self.started.send(cancellation.clone());
        async move {
            let _owner = owner;
            std::future::pending::<JobOutcome>().await
        }
    }
}

struct AssignmentTransport {
    assignment_available: AtomicBool,
    sent: Mutex<Vec<WorkerProtocolMessage>>,
}

impl AssignmentTransport {
    fn new() -> Self {
        Self {
            assignment_available: AtomicBool::new(true),
            sent: Mutex::new(Vec::new()),
        }
    }
}

impl Transport for AssignmentTransport {
    fn send(
        &self,
        _cx: Cx,
        message: WorkerProtocolMessage,
        _auth: Option<WorkerAuth>,
    ) -> impl Future<Output = Result<Option<WorkerProtocolMessage>, String>> + Send {
        self.sent
            .lock()
            .expect("sent messages")
            .push(message.clone());
        let response = match message {
            WorkerProtocolMessage::Poll(_)
                if self.assignment_available.swap(false, Ordering::AcqRel) =>
            {
                Some(WorkerProtocolMessage::Assign(assignment()))
            }
            _ => None,
        };
        async move { Ok(response) }
    }
}

fn assignment() -> Assign {
    Assign {
        protocol_version: WORKER_PROTOCOL_VERSION,
        trace_context: None,
        job_id: "active-job".to_string(),
        attempt_id: Some("active-attempt".to_string()),
        role: "engineer".to_string(),
        repo: "ai/temper".to_string(),
        artifact: Artifact {
            item: serde_json::json!(454),
            kind: "issue".to_string(),
        },
        job_payload: serde_json::json!({}),
    }
}

fn config(result_root: std::path::PathBuf) -> WorkerConfig {
    WorkerConfig {
        daemon_url: String::new(),
        worker_id: "shutdown-worker".to_string(),
        worker_pool: None,
        worker_auth: None,
        capabilities: Vec::new(),
        role_identities: BTreeMap::new(),
        max_concurrent_jobs: 1,
        poll_wait: Duration::from_millis(1),
        heartbeat_interval: Duration::from_secs(60),
        liveness_limits: WorkerLivenessLimits {
            max_no_progress: Duration::from_secs(60),
            max_run: None,
            graceful_cancellation_grace: Duration::from_millis(10),
            forced_termination_grace: Duration::from_millis(10),
        },
        result_root,
        agent_traces: WorkerAgentTraceConfig::default(),
        executor: ExecutorSelection::Stub,
    }
}

fn run_stop_scenario(shutdown: WorkerShutdown, finish_at: crate::JobCancellationRequest) {
    temper_worker_io::block_on_with(move |_cx, handle| async move {
        let temp = tempfile::tempdir().expect("tempdir");
        let (started_tx, mut started_rx) = channel();
        let finished = Arc::new(AtomicBool::new(false));
        let executor = Arc::new(ControlledExecutor {
            started: started_tx,
            finished: Arc::clone(&finished),
            finish_at,
        });
        let transport = Arc::new(AssignmentTransport::new());
        let worker = start_worker_with_transport(
            handle,
            config(temp.path().join("results")),
            executor,
            Arc::clone(&transport),
        );
        let registry = worker.task_registry();
        let cancellation = started_rx.recv().await.expect("job started");
        assert_eq!(registry.active_jobs().len(), 1);

        match shutdown {
            WorkerShutdown::Graceful => worker.shutdown().await,
            WorkerShutdown::Crash => worker.crash().await,
        }

        let expected = match (shutdown, finish_at) {
            (WorkerShutdown::Graceful, crate::JobCancellationRequest::Graceful) => {
                crate::JobCancellationRequest::Graceful
            }
            _ => crate::JobCancellationRequest::HardKill,
        };
        assert_eq!(cancellation.requested(), Some(expected));
        assert!(finished.load(Ordering::Acquire));
        assert!(registry.is_empty());
        assert!(
            transport
                .sent
                .lock()
                .expect("sent messages")
                .iter()
                .all(|message| !matches!(message, WorkerProtocolMessage::Result(_))),
            "component stop must preserve the active durable claim"
        );
    });
}

#[test]
fn bounded_shutdown_reports_and_retains_an_unresolved_attempt() {
    temper_worker_io::block_on_with(move |_cx, handle| async move {
        let temp = tempfile::tempdir().expect("tempdir");
        let (started_tx, mut started_rx) = channel();
        let transport = Arc::new(AssignmentTransport::new());
        let mut worker = start_worker_with_transport(
            handle,
            config(temp.path().join("results")),
            Arc::new(BlockingExecutor {
                started: started_tx,
            }),
            Arc::clone(&transport),
        );
        let registry = worker.task_registry();
        let registry_after_fence = registry.clone();
        let cancellation = started_rx.recv().await.expect("job started");
        let report = worker
            .shutdown_bounded_after_fence(
                std::time::Instant::now() + Duration::from_millis(30),
                move || {
                    assert!(registry_after_fence.is_shutting_down());
                    let active = registry_after_fence.active_jobs();
                    assert_eq!(active.len(), 1);
                    assert!(
                        !active[0].fence().is_open(),
                        "HTTP drain callback must follow AttemptFence closure"
                    );
                },
            )
            .await;

        assert!(report.joined_attempts.is_empty());
        let blocker = report
            .unresolved_blockers
            .iter()
            .find(|blocker| blocker.kind == crate::ShutdownBlockerKind::RegistryState)
            .expect("registry blocker");
        assert_eq!(blocker.worker_id.as_deref(), Some("shutdown-worker"));
        assert_eq!(blocker.job_id.as_deref(), Some("active-job"));
        assert_eq!(blocker.attempt_id.as_deref(), Some("active-attempt"));
        assert_eq!(blocker.owner_name, "hard_kill_requested");
        assert_eq!(
            blocker.escalation_stage,
            crate::ShutdownEscalationStage::HardKill
        );
        assert_eq!(
            cancellation.requested(),
            Some(crate::JobCancellationRequest::HardKill)
        );
        assert_eq!(registry.active_jobs().len(), 1);
        assert!(
            transport
                .sent
                .lock()
                .expect("sent messages")
                .iter()
                .all(|message| !matches!(message, WorkerProtocolMessage::Result(_))),
            "deadline expiry must not publish a result"
        );
    });
}

#[test]
fn shutdown_joins_active_job_without_publishing_a_cancellation_result() {
    run_stop_scenario(
        WorkerShutdown::Graceful,
        crate::JobCancellationRequest::Graceful,
    );
}

#[test]
fn shutdown_applies_forced_and_hard_deadlines_before_joining() {
    run_stop_scenario(
        WorkerShutdown::Graceful,
        crate::JobCancellationRequest::HardKill,
    );
}

#[test]
fn crash_hard_escalates_joins_and_preserves_the_active_claim() {
    run_stop_scenario(
        WorkerShutdown::Crash,
        crate::JobCancellationRequest::HardKill,
    );
}

#[derive(Clone, Default)]
struct SharedLogBuffer(Arc<Mutex<Vec<u8>>>);

impl io::Write for SharedLogBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().expect("log buffer").write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for SharedLogBuffer {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

struct StartupRecoveryTransport {
    collector: TraceCollector,
}

impl Transport for StartupRecoveryTransport {
    fn send(
        &self,
        _cx: Cx,
        message: WorkerProtocolMessage,
        _auth: Option<WorkerAuth>,
    ) -> impl Future<Output = Result<Option<WorkerProtocolMessage>, String>> + Send {
        if matches!(
            message,
            WorkerProtocolMessage::Register(_) | WorkerProtocolMessage::Poll(_)
        ) {
            let inventory = self.collector.inventory().expect("inventory at intake");
            assert_eq!(
                inventory.outcomes.abandoned_non_terminal_runs, 0,
                "startup recovery must finish before registration or polling"
            );
        }
        async { Ok(None) }
    }
}

struct BackgroundRecoveryTransport {
    collector: TraceCollector,
    assignment_available: AtomicBool,
    dirty_at_first_poll: Arc<AtomicU64>,
}

impl Transport for BackgroundRecoveryTransport {
    fn send(
        &self,
        _cx: Cx,
        message: WorkerProtocolMessage,
        _auth: Option<WorkerAuth>,
    ) -> impl Future<Output = Result<Option<WorkerProtocolMessage>, String>> + Send {
        let response = match message {
            WorkerProtocolMessage::Poll(_)
                if self.assignment_available.swap(false, Ordering::AcqRel) =>
            {
                let inventory = self.collector.inventory().expect("background inventory");
                let remaining = inventory
                    .dirty_run_count
                    .saturating_add(inventory.outcomes.malformed_runs);
                self.dirty_at_first_poll.store(remaining, Ordering::Release);
                Some(WorkerProtocolMessage::Assign(assignment()))
            }
            _ => None,
        };
        async move { Ok(response) }
    }
}

fn trace_context() -> WorkspaceContext {
    WorkspaceContext {
        trace_context: None,
        repos: vec![WorkspaceRepository {
            id: "forgejo:ai/temper".to_string(),
            owner: "ai".to_string(),
            name: "temper".to_string(),
            default_branch: "main".to_string(),
            dir: "temper".to_string(),
            access: "writable".to_string(),
            base_branch: "main".to_string(),
            branch_hint: Some("agent/startup-recovery".to_string()),
        }],
        work_item: WorkspaceWorkItem {
            role: "engineer".to_string(),
            queue: "code_ready".to_string(),
            kind: "code".to_string(),
            target: "Issue { number: ItemNumber(746) }".to_string(),
            context: "{}".to_string(),
        },
        artifact_context: None,
        action: "open_pr".to_string(),
        correlation_key: "pr-for-code-746".to_string(),
        checkout: Some("writable".to_string()),
        allowed_verdicts: Vec::new(),
        verdict_contracts: Default::default(),
        source_metadata: Default::default(),
        guidance: Default::default(),
        pull_request_freshness: None,
        agent_session: Some(AgentSessionState::new("startup-recovery-session")),
    }
}

fn trace_config(root: std::path::PathBuf) -> WorkerAgentTraceConfig {
    WorkerAgentTraceConfig {
        policy: AgentActivityCapturePolicyV1 {
            max_run_bytes: 5_000,
            max_inline_bytes: 1,
            max_blob_bytes: 1,
            ..Default::default()
        },
        spool_root: Some(root),
    }
}

#[test]
fn startup_reclaims_a_saturated_sixteen_run_spool_before_intake() {
    temper_worker_io::block_on_with(move |_cx, handle| async move {
        let temp = tempfile::tempdir().expect("tempdir");
        let traces = trace_config(temp.path().join("traces"));
        let collector = TraceCollector::new(traces.clone());
        let context = trace_context();
        let mut stale = Vec::new();
        for index in 0..STARTUP_TRACE_RECLAMATION_RUN_BUDGET {
            stale.push(
                collector
                    .begin_run(&format!("stale-{index}"), &context)
                    .expect("seed stale reservation")
                    .expect("trace enabled"),
            );
        }
        drop(stale);

        let human = SharedLogBuffer::default();
        let json = SharedLogBuffer::default();
        let subscriber = tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .without_time()
                    .with_writer(human.clone()),
            )
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_writer(json.clone()),
            );
        let worker_config = WorkerConfig {
            agent_traces: traces,
            ..config(temp.path().join("results"))
        };
        let worker = tracing::subscriber::with_default(subscriber, || {
            start_worker_with_transport_and_trace_collector(
                handle,
                worker_config,
                Arc::new(ControlledExecutor {
                    started: channel().0,
                    finished: Arc::new(AtomicBool::new(false)),
                    finish_at: crate::JobCancellationRequest::Graceful,
                }),
                Arc::new(StartupRecoveryTransport {
                    collector: collector.clone(),
                }),
                collector.clone(),
            )
        });

        let inventory = collector.inventory().expect("post-start inventory");
        assert_eq!(inventory.outcomes.abandoned_non_terminal_runs, 0);
        let next = collector
            .begin_run("admitted-after-recovery", &context)
            .expect("startup reclaimed logical capacity")
            .expect("trace enabled");
        next.finish_success(None).expect("finish admitted trace");
        drop(next);

        let records =
            String::from_utf8(json.0.lock().expect("json logs").clone()).expect("UTF-8 logs");
        let summary = records
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("JSON event"))
            .find(|record| record["fields"]["event"] == "agent.activity.startup_recovery")
            .expect("startup recovery summary");
        assert_eq!(summary["fields"]["terminalized_runs"], 16);
        assert_eq!(summary["fields"]["quarantined_runs"], 0);
        assert_eq!(summary["fields"]["protected_runs"], 0);
        assert_eq!(summary["fields"]["failed_runs"], 0);
        assert_eq!(summary["fields"]["remaining_dirty_runs"], 0);
        let physical = summary["fields"]["physical_used_bytes"]
            .as_u64()
            .expect("physical total");
        let logical = summary["fields"]["logical_reserved_bytes"]
            .as_u64()
            .expect("logical total");
        let rendered =
            String::from_utf8(human.0.lock().expect("human logs").clone()).expect("UTF-8 logs");
        assert!(rendered.contains(&format!(
            "terminalized 16, quarantined 0, protected 0, failed 0, remaining dirty 0, physical used bytes {physical}, logical reserved bytes {logical}"
        )));

        worker.shutdown().await;
    });
}

#[test]
fn startup_recovery_failure_still_allows_assignment_intake() {
    temper_worker_io::block_on_with(move |_cx, handle| async move {
        let temp = tempfile::tempdir().expect("tempdir");
        let invalid_root = temp.path().join("trace-root-is-a-file");
        std::fs::write(&invalid_root, b"not a spool directory").expect("invalid root");
        let (started_tx, mut started_rx) = channel();
        let finished = Arc::new(AtomicBool::new(false));
        let traces = trace_config(invalid_root);
        let collector = TraceCollector::new(traces.clone());
        let worker = start_worker_with_transport_and_trace_collector(
            handle,
            WorkerConfig {
                agent_traces: traces,
                ..config(temp.path().join("results"))
            },
            Arc::new(ControlledExecutor {
                started: started_tx,
                finished: Arc::clone(&finished),
                finish_at: crate::JobCancellationRequest::Graceful,
            }),
            Arc::new(AssignmentTransport::new()),
            collector,
        );
        started_rx
            .recv()
            .await
            .expect("assignment starts after recovery failure");
        worker.shutdown().await;
        assert!(finished.load(Ordering::Acquire));
    });
}

#[test]
fn bounded_background_recovery_yields_to_assignments_and_joins_on_shutdown() {
    temper_worker_io::block_on_with(move |_cx, handle| async move {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("traces");
        std::fs::create_dir_all(&root).expect("trace root");
        for index in 0..=STARTUP_TRACE_RECLAMATION_RUN_BUDGET {
            std::fs::create_dir(root.join(format!("malformed-{index:02}")))
                .expect("malformed spool");
        }
        let traces = trace_config(root);
        let collector = TraceCollector::new(traces.clone());
        let dirty_at_first_poll = Arc::new(AtomicU64::new(u64::MAX));
        let transport = Arc::new(BackgroundRecoveryTransport {
            collector: collector.clone(),
            assignment_available: AtomicBool::new(true),
            dirty_at_first_poll: Arc::clone(&dirty_at_first_poll),
        });
        let (started_tx, mut started_rx) = channel();
        let finished = Arc::new(AtomicBool::new(false));
        let worker = start_worker_with_transport_and_trace_collector(
            handle,
            WorkerConfig {
                agent_traces: traces,
                ..config(temp.path().join("results"))
            },
            Arc::new(ControlledExecutor {
                started: started_tx,
                finished: Arc::clone(&finished),
                finish_at: crate::JobCancellationRequest::Graceful,
            }),
            transport,
            collector.clone(),
        );
        started_rx
            .recv()
            .await
            .expect("assignment starts before background convergence");
        assert_eq!(dirty_at_first_poll.load(Ordering::Acquire), 1);

        let mut converged = false;
        for _ in 0..20 {
            let inventory = collector.inventory().expect("recovery inventory");
            if inventory.dirty_run_count == 0 && inventory.outcomes.malformed_runs == 0 {
                converged = true;
                break;
            }
            temper_worker_io::sleep_for(Duration::from_millis(25)).await;
        }
        assert!(
            converged,
            "background recovery must converge in bounded passes"
        );
        worker.shutdown().await;
        assert!(finished.load(Ordering::Acquire));
    });
}
