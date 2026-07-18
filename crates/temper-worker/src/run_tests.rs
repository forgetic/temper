use std::collections::BTreeMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use skein::cx::Cx;
use temper_protocol_worker::{
    Artifact, Assign, FailureClass, WORKER_PROTOCOL_VERSION, WorkerAuth, WorkerProtocolMessage,
};
use temper_worker_io::{CqSender, channel};

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
