use std::time::Duration;

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::json;
use smith_worker::{CapabilitySpec, ExecutorSelection, StubExecutor, WorkerConfig, run_worker};
use temper_worker_protocol::{
    Artifact, Assign, ErrorCode, FailureClass, JobResult, ProtocolError, Register, Release,
    ReleaseDisposition, ResultStatus, WORKER_PROTOCOL_VERSION, WorkerProtocolMessage,
};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};

#[derive(Clone)]
struct AppState {
    requests: mpsc::Sender<DaemonRequest>,
}

struct DaemonRequest {
    message: WorkerProtocolMessage,
    reply: oneshot::Sender<DaemonReply>,
}

#[derive(Debug)]
enum DaemonReply {
    NoContent,
    Message(Box<WorkerProtocolMessage>),
}

#[derive(Debug)]
struct ObservedRun {
    registers: Vec<Register>,
    result: JobResult,
}

async fn message_handler(
    State(state): State<AppState>,
    Json(message): Json<WorkerProtocolMessage>,
) -> Response {
    let (reply_tx, reply_rx) = oneshot::channel();
    if state
        .requests
        .send(DaemonRequest {
            message,
            reply: reply_tx,
        })
        .await
        .is_err()
    {
        return (StatusCode::INTERNAL_SERVER_ERROR, "fake daemon stopped").into_response();
    }

    match reply_rx.await {
        Ok(DaemonReply::NoContent) => StatusCode::NO_CONTENT.into_response(),
        Ok(DaemonReply::Message(reply)) => Json(*reply).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "fake daemon dropped reply",
        )
            .into_response(),
    }
}

async fn spawn_fake_daemon(config: &WorkerConfig) -> (String, oneshot::Receiver<ObservedRun>) {
    let (request_tx, request_rx) = mpsc::channel(16);
    let (observed_tx, observed_rx) = oneshot::channel();
    let assign = Assign {
        protocol_version: WORKER_PROTOCOL_VERSION,
        job_id: "job-123".to_string(),
        role: config.capabilities[0].role.clone(),
        repo: config.capabilities[0].repo.clone(),
        artifact: Artifact {
            item: json!(78),
            kind: "intake".to_string(),
        },
        job_payload: json!({}),
    };
    tokio::spawn(fake_daemon_controller(request_rx, observed_tx, assign));

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake daemon");
    let addr = listener.local_addr().expect("fake daemon local address");
    let app = Router::new()
        .route("/v1/message", post(message_handler))
        .with_state(AppState {
            requests: request_tx,
        });
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("fake daemon serves");
    });

    (format!("http://{addr}"), observed_rx)
}

async fn fake_daemon_controller(
    mut requests: mpsc::Receiver<DaemonRequest>,
    observed: oneshot::Sender<ObservedRun>,
    assign: Assign,
) {
    let mut registers = Vec::new();
    let mut assigned = false;
    let mut observed = Some(observed);

    while let Some(request) = requests.recv().await {
        match request.message {
            WorkerProtocolMessage::Register(message) => {
                registers.push(message);
                request
                    .reply
                    .send(DaemonReply::NoContent)
                    .expect("handler receives no-content reply");
            }
            WorkerProtocolMessage::Poll(_) if !assigned => {
                assigned = true;
                request
                    .reply
                    .send(DaemonReply::Message(Box::new(
                        WorkerProtocolMessage::Assign(assign.clone()),
                    )))
                    .expect("handler receives assign reply");
            }
            WorkerProtocolMessage::Poll(_) => reply_with_timeout(request.reply),
            WorkerProtocolMessage::Heartbeat(_) => reply_with_timeout(request.reply),
            WorkerProtocolMessage::Result(result) => {
                let release = WorkerProtocolMessage::Release(Release {
                    protocol_version: WORKER_PROTOCOL_VERSION,
                    worker_id: result.worker_id.clone(),
                    job_id: result.job_id.clone(),
                    disposition: ReleaseDisposition::Accepted,
                    message: None,
                });
                request
                    .reply
                    .send(DaemonReply::Message(Box::new(release)))
                    .expect("handler receives release reply");

                if let Some(observed) = observed.take() {
                    observed
                        .send(ObservedRun {
                            registers: registers.clone(),
                            result,
                        })
                        .expect("test receives observed run");
                }
            }
            other => {
                request
                    .reply
                    .send(DaemonReply::Message(Box::new(
                        WorkerProtocolMessage::Error(ProtocolError {
                            protocol_version: WORKER_PROTOCOL_VERSION,
                            code: ErrorCode::MalformedMessage,
                            message: format!("unexpected fake-daemon request: {other:?}"),
                            retry_after_ms: None,
                            job_id: None,
                        }),
                    )))
                    .expect("handler receives error reply");
            }
        }
    }
}

fn reply_with_timeout(reply: oneshot::Sender<DaemonReply>) {
    reply
        .send(DaemonReply::Message(Box::new(
            WorkerProtocolMessage::Error(ProtocolError {
                protocol_version: WORKER_PROTOCOL_VERSION,
                code: ErrorCode::PollTimeout,
                message: "no assignment available before the long-poll deadline".to_string(),
                retry_after_ms: None,
                job_id: None,
            }),
        )))
        .expect("handler receives timeout reply");
}

fn worker_config() -> WorkerConfig {
    WorkerConfig {
        daemon_url: "http://127.0.0.1:0".to_string(),
        worker_id: "worker-1".to_string(),
        capabilities: vec![CapabilitySpec {
            repo: "ai/smith".to_string(),
            role: "engineer".to_string(),
        }],
        max_concurrent_jobs: 1,
        poll_wait: Duration::from_millis(25),
        heartbeat_interval: Duration::from_millis(25),
        executor: ExecutorSelection::Stub,
    }
}

/// Run the worker on its own single-threaded skein runtime in a dedicated
/// thread. The worker now requires an skein runtime to host its sans-IO
/// drive loop (and, in production, in-process agent jobs), so it cannot run as a
/// tokio task. The fake daemon stays on tokio; the two communicate over real
/// HTTP, exactly as in production. The thread is detached — the worker loops
/// forever, and the test returns once the daemon has observed the result.
fn spawn_worker_thread<E>(config: WorkerConfig, executor: std::sync::Arc<E>)
where
    E: smith_worker::JobExecutor + Send + Sync + 'static,
{
    std::thread::spawn(move || {
        let _ = smith_io_engine::block_on(async move { run_worker(config, executor).await });
    });
}

#[tokio::test]
async fn success_stub_registers_polls_runs_and_posts_result() {
    let mut config = worker_config();
    let (daemon_url, observed) = spawn_fake_daemon(&config).await;
    config.daemon_url = daemon_url;

    spawn_worker_thread(config.clone(), StubExecutor::success().into());
    let observed = tokio::time::timeout(Duration::from_secs(5), observed)
        .await
        .expect("fake daemon observes success result")
        .expect("fake daemon sends observed run");

    assert_eq!(observed.registers.len(), 1);
    assert_eq!(observed.registers[0].worker_id, config.worker_id);
    assert_eq!(observed.registers[0].capabilities.len(), 1);
    assert_eq!(observed.registers[0].capabilities[0].repo, "ai/smith");
    assert_eq!(observed.registers[0].capabilities[0].role, "engineer");
    assert_eq!(observed.registers[0].capacity.max_concurrent_jobs, 1);

    assert_eq!(observed.result.job_id, "job-123");
    assert_eq!(observed.result.status, ResultStatus::Success);
    assert!(observed.result.branch.is_some());
    assert_eq!(observed.result.failure, None);
}

#[tokio::test]
async fn failure_stub_registers_polls_runs_and_posts_failure_result() {
    let mut config = worker_config();
    let (daemon_url, observed) = spawn_fake_daemon(&config).await;
    config.daemon_url = daemon_url;

    spawn_worker_thread(
        config,
        StubExecutor::failure(FailureClass::Permanent, "configured failure").into(),
    );
    let observed = tokio::time::timeout(Duration::from_secs(5), observed)
        .await
        .expect("fake daemon observes failure result")
        .expect("fake daemon sends observed run");

    assert_eq!(observed.result.job_id, "job-123");
    assert_eq!(observed.result.status, ResultStatus::Failure);
    assert_eq!(observed.result.branch, None);
    let failure = observed.result.failure.expect("failure details present");
    assert_eq!(failure.class, FailureClass::Permanent);
    assert_eq!(failure.message, "configured failure");
}
