//! Hermetic full-worker end-to-end test of the **out-of-process agent boundary**.
//!
//! A real `smith-worker` runs a real coding job by spawning a real agent
//! **process** (`smith-fake-agent`, a deterministic protocol speaker — no LLM,
//! no git), with NO external Forgejo or runner. This exercises the orchestration
//! path and the `smith-agent-protocol` boundary; the agent's own LLM loop is
//! tested in the `anvil` repo against jig.
//!
//! Topology, all in one process:
//! - a **fake daemon** (tokio axum) speaks the worker/daemon wire protocol:
//!   accepts register, assigns one real coding job (a full `WireJobContext`
//!   payload), then accepts the result;
//! - a **real `smith-worker`** runs on its own skein runtime thread with an
//!   [`OutOfProcessRunner`] pointed at the `smith-fake-agent` binary;
//! - a **recording [`ProgressSink`]** captures every step-progress marker the
//!   worker relayed from the agent's stdout;
//! - git remotes are local `file://` bare repos seeded with an initial commit.
//!
//! This drives the entire production path: worker register → poll → assign →
//! `CodingExecutor` prepares the checkout → spawns `smith-fake-agent` over the
//! protocol → the agent writes the product file + emits step-progress → the
//! worker relays progress, commits and pushes the branch to the `file://` origin
//! → reports Success. Assertions verify the branch landed with the agent's file,
//! the worker reported Success, and the step-progress checkpoints were relayed.
//!
//! A second test injects an agent crash *after* it emits progress but *before* it
//! writes the result — the crash-recovery scenario: the worker reports a
//! transient failure (re-dispatchable) and the already-emitted progress markers
//! were still relayed.
//!
//! Hermetic and fast; runs by default.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::json;
use smith_agent_protocol::{StepProgress, StepState};
use smith_worker::config::CapabilitySpec;
use smith_worker::{
    CodingExecutor, CodingExecutorConfig, ExecutorSelection, OutOfProcessRunner, ProgressSink,
    RoleGitIdentity, WorkerConfig, run_worker,
};
use temper_worker_protocol::{
    Artifact, Assign, ProtocolError, Release, ReleaseDisposition, ResultStatus,
    WORKER_PROTOCOL_VERSION, WorkerProtocolMessage,
};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};

/// Records every step-progress marker the worker relays, so the test can assert
/// the agent→worker→sink path fired.
#[derive(Clone, Default)]
struct RecordingProgressSink {
    markers: Arc<Mutex<Vec<StepProgress>>>,
}

impl ProgressSink for RecordingProgressSink {
    fn report(&self, progress: StepProgress) {
        self.markers.lock().expect("markers lock").push(progress);
    }
}

impl RecordingProgressSink {
    fn snapshot(&self) -> Vec<StepProgress> {
        self.markers.lock().expect("markers lock").clone()
    }
}

#[tokio::test]
async fn worker_runs_a_real_coding_job_through_the_out_of_process_agent() {
    let fixture = GitFixture::new();

    let (daemon_url, observed) = spawn_fake_daemon(&fixture).await;
    let config = WorkerConfig {
        daemon_url,
        ..worker_config()
    };

    // The out-of-process runner spawns the deterministic fake agent binary,
    // which writes GREETING.md and emits two step-progress markers.
    let runner = Arc::new(OutOfProcessRunner::new(vec![fake_agent_bin()]));
    let executor_config = CodingExecutorConfig {
        workspace_root: fixture.workspace_root.clone(),
        git_base_url: fixture.git_base_url(),
        role_identities: role_identities(),
    };
    let sink = RecordingProgressSink::default();
    let executor = Arc::new(
        CodingExecutor::new(executor_config, runner).with_progress_sink(Arc::new(sink.clone())),
    );

    spawn_worker_thread(config, executor);

    let observed = tokio::time::timeout(std::time::Duration::from_secs(30), observed)
        .await
        .expect("daemon observes a result within the timeout")
        .expect("daemon sends the observed run");

    // The worker reported Success with a branch.
    assert_eq!(
        observed.result.status,
        ResultStatus::Success,
        "result: {:?}",
        observed.result
    );
    let branch = observed.result.branch.expect("success carries a branch");
    assert_eq!(branch.name, "agent/pr-for-code-7");
    assert_eq!(branch.head_sha.len(), 40, "head sha looks like a real sha");

    // The branch landed on origin with the agent's product file.
    let pushed_sha = fixture.origin_rev("refs/heads/agent/pr-for-code-7");
    assert_eq!(
        pushed_sha, branch.head_sha,
        "the reported sha is what was pushed"
    );
    let greeting = fixture.origin_show("refs/heads/agent/pr-for-code-7:GREETING.md");
    assert_eq!(
        greeting, "hello from the fake agent",
        "the agent's product file was committed and pushed"
    );
    // The commit message carries the correlation key + Closes trailer.
    assert_eq!(
        fixture.origin_log_format("refs/heads/agent/pr-for-code-7", "%s"),
        "Implement pr-for-code-7"
    );
    assert_eq!(
        fixture.origin_log_format("refs/heads/agent/pr-for-code-7", "%b"),
        "Closes #7"
    );

    // The worker relayed the agent's step-progress checkpoints (the crash-recovery
    // channel): a Started marker and a Done marker, both stamped with the job's
    // correlation key.
    let markers = sink.snapshot();
    assert_eq!(
        markers.len(),
        2,
        "expected two step-progress markers: {markers:?}"
    );
    assert!(markers.iter().all(|m| m.correlation_key == "pr-for-code-7"));
    assert_eq!(markers[0].state, StepState::Started);
    assert_eq!(markers[1].state, StepState::Done);
}

#[tokio::test]
async fn worker_reports_transient_failure_when_agent_crashes_after_progress() {
    let fixture = GitFixture::new();

    let (daemon_url, observed) = spawn_fake_daemon(&fixture).await;
    let config = WorkerConfig {
        daemon_url,
        ..worker_config()
    };

    // The fake agent emits progress, then exits non-zero before writing a result
    // — a crash mid-task. (A future slice has the agent push its partial work
    // first so the next agent resumes; here we assert the worker's handling: the
    // emitted markers were relayed, and the job is a re-dispatchable transient.)
    // The crash knob is a command arg, not an env var, so concurrent test
    // threads cannot race on it.
    let runner = Arc::new(OutOfProcessRunner::new(vec![
        fake_agent_bin(),
        "--crash-after-progress".to_string(),
    ]));
    let executor_config = CodingExecutorConfig {
        workspace_root: fixture.workspace_root.clone(),
        git_base_url: fixture.git_base_url(),
        role_identities: role_identities(),
    };
    let sink = RecordingProgressSink::default();
    let executor = Arc::new(
        CodingExecutor::new(executor_config, runner).with_progress_sink(Arc::new(sink.clone())),
    );

    spawn_worker_thread(config, executor);

    let observed = tokio::time::timeout(std::time::Duration::from_secs(30), observed)
        .await
        .expect("daemon observes a result within the timeout")
        .expect("daemon sends the observed run");

    // The worker reported a transient failure (the crash is re-dispatchable).
    assert_eq!(
        observed.result.status,
        ResultStatus::Failure,
        "result: {:?}",
        observed.result
    );
    let failure = observed.result.failure.expect("failure carries detail");
    assert_eq!(
        failure.class,
        temper_worker_protocol::FailureClass::Transient
    );

    // The progress the agent emitted *before* crashing was still relayed — the
    // recovery channel survives the crash.
    let markers = sink.snapshot();
    assert!(
        markers.iter().any(|m| m.state == StepState::Started),
        "the pre-crash Started marker must have been relayed: {markers:?}"
    );
}

// ---------------------------------------------------------------------------
// The fake agent binary path (built by cargo as a [[bin]] of this crate).
// ---------------------------------------------------------------------------

fn fake_agent_bin() -> String {
    env!("CARGO_BIN_EXE_smith-fake-agent").to_string()
}

// ---------------------------------------------------------------------------
// Worker config + identities.
// ---------------------------------------------------------------------------

fn worker_config() -> WorkerConfig {
    WorkerConfig {
        daemon_url: "http://placeholder".to_string(),
        worker_id: "coding-worker-e2e".to_string(),
        capabilities: vec![CapabilitySpec {
            repo: "acme/service".to_string(),
            role: "engineer".to_string(),
        }],
        max_concurrent_jobs: 1,
        poll_wait: std::time::Duration::from_millis(50),
        heartbeat_interval: std::time::Duration::from_millis(50),
        // `run_worker` takes the executor we construct directly, so the config's
        // `executor` field is unused here (it only matters to the binary's arg
        // parsing); leave it as the stub shape.
        executor: ExecutorSelection::Stub,
    }
}

fn role_identities() -> BTreeMap<String, RoleGitIdentity> {
    let mut m = BTreeMap::new();
    m.insert(
        "engineer".to_string(),
        RoleGitIdentity {
            user: "Smith Engineer".to_string(),
            email: "engineer@example.test".to_string(),
            token: "test-token".to_string(),
        },
    );
    m
}

fn spawn_worker_thread<E>(config: WorkerConfig, executor: Arc<E>)
where
    E: smith_worker::JobExecutor + Send + Sync + 'static,
{
    std::thread::spawn(move || {
        let _ = smith_io_engine::block_on(async move { run_worker(config, executor).await });
    });
}

// ---------------------------------------------------------------------------
// Fake daemon: assign one coding job, accept the result.
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct AppState {
    requests: mpsc::Sender<DaemonRequest>,
}

struct DaemonRequest {
    message: WorkerProtocolMessage,
    reply: oneshot::Sender<DaemonReply>,
}

enum DaemonReply {
    NoContent,
    Message(Box<WorkerProtocolMessage>),
}

struct ObservedRun {
    result: temper_worker_protocol::JobResult,
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

async fn spawn_fake_daemon(fixture: &GitFixture) -> (String, oneshot::Receiver<ObservedRun>) {
    let (request_tx, request_rx) = mpsc::channel(16);
    let (observed_tx, observed_rx) = oneshot::channel();
    let assign = coding_assign(fixture);
    tokio::spawn(fake_daemon_controller(request_rx, observed_tx, assign));

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake daemon");
    let addr = listener.local_addr().expect("fake daemon addr");
    let app = Router::new()
        .route("/v1/message", post(message_handler))
        .with_state(AppState {
            requests: request_tx,
        });
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    (format!("http://{addr}"), observed_rx)
}

async fn fake_daemon_controller(
    mut requests: mpsc::Receiver<DaemonRequest>,
    observed: oneshot::Sender<ObservedRun>,
    assign: Assign,
) {
    let mut assigned = false;
    let mut observed = Some(observed);
    while let Some(request) = requests.recv().await {
        match request.message {
            WorkerProtocolMessage::Register(_) => {
                let _ = request.reply.send(DaemonReply::NoContent);
            }
            WorkerProtocolMessage::Poll(_) if !assigned => {
                assigned = true;
                let _ = request.reply.send(DaemonReply::Message(Box::new(
                    WorkerProtocolMessage::Assign(assign.clone()),
                )));
            }
            WorkerProtocolMessage::Poll(_) | WorkerProtocolMessage::Heartbeat(_) => {
                let _ = request
                    .reply
                    .send(DaemonReply::Message(Box::new(poll_timeout())));
            }
            WorkerProtocolMessage::Result(result) => {
                let release = WorkerProtocolMessage::Release(Release {
                    protocol_version: WORKER_PROTOCOL_VERSION,
                    worker_id: result.worker_id.clone(),
                    job_id: result.job_id.clone(),
                    disposition: ReleaseDisposition::Accepted,
                    message: None,
                });
                let _ = request.reply.send(DaemonReply::Message(Box::new(release)));
                if let Some(observed) = observed.take() {
                    let _ = observed.send(ObservedRun { result });
                }
            }
            other => {
                let _ = request.reply.send(DaemonReply::Message(Box::new(
                    WorkerProtocolMessage::Error(ProtocolError {
                        protocol_version: WORKER_PROTOCOL_VERSION,
                        code: temper_worker_protocol::ErrorCode::MalformedMessage,
                        message: format!("unexpected: {other:?}"),
                        retry_after_ms: None,
                        job_id: None,
                    }),
                )));
            }
        }
    }
}

fn poll_timeout() -> WorkerProtocolMessage {
    WorkerProtocolMessage::Error(ProtocolError {
        protocol_version: WORKER_PROTOCOL_VERSION,
        code: temper_worker_protocol::ErrorCode::PollTimeout,
        message: "no work".to_string(),
        retry_after_ms: None,
        job_id: None,
    })
}

/// A full coding-job Assign payload (the enriched WireJobContext the executor
/// requires).
fn coding_assign(_fixture: &GitFixture) -> Assign {
    let job_context = json!({
        "role": "engineer",
        "repo": "acme/service",
        "queue": "code_ready",
        "artifact_kind": "code",
        "repository": {
            "owner": "acme",
            "name": "service",
            "default_branch": "main"
        },
        "base_branch": "main",
        "branch_hint": "agent/pr-for-code-7",
        "correlation_key": "pr-for-code-7",
        "artifact": {
            "number": 7,
            "title": "Add a greeting file",
            "body": "Create GREETING.md.",
            "labels": ["code", "ready"],
            "state": "Open"
        },
        "action": "open_pr",
        "checkout_capability": "writable",
        "allowed_verdicts": []
    });
    Assign {
        protocol_version: WORKER_PROTOCOL_VERSION,
        job_id: "acme/service/issue-7/engineer/pr-for-code-7".to_string(),
        role: "engineer".to_string(),
        repo: "acme/service".to_string(),
        artifact: Artifact {
            item: json!(7),
            kind: "issue".to_string(),
        },
        job_payload: job_context,
    }
}

// ---------------------------------------------------------------------------
// Git fixture (bare origin seeded with main, file:// remotes).
// ---------------------------------------------------------------------------

struct GitFixture {
    temp: tempfile::TempDir,
    origin: PathBuf,
    workspace_root: PathBuf,
}

impl GitFixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("temp dir");
        let git_root = temp.path().join("git");
        fs::create_dir_all(git_root.join("acme")).expect("git root");
        let origin = git_root.join("acme/service.git");
        git(["init", "--bare", path_str(&origin)]);
        seed_origin(&origin, temp.path());
        let workspace_root = temp.path().join("workspaces");
        Self {
            temp,
            origin,
            workspace_root,
        }
    }

    fn git_base_url(&self) -> String {
        format!("file://{}/git", path_str(self.temp.path()))
    }

    fn origin_rev(&self, refname: &str) -> String {
        git_output(["-C", path_str(&self.origin), "rev-parse", refname])
    }

    fn origin_show(&self, spec: &str) -> String {
        git_output(["-C", path_str(&self.origin), "show", spec])
    }

    fn origin_log_format(&self, refname: &str, fmt: &str) -> String {
        git_output([
            "-C",
            path_str(&self.origin),
            "log",
            "-1",
            &format!("--format={fmt}"),
            refname,
        ])
    }
}

fn seed_origin(origin: &Path, temp: &Path) {
    let seed = temp.join("seed");
    git(["init", "-b", "main", path_str(&seed)]);
    fs::write(seed.join("README.md"), "# seed\n").expect("seed file");
    git([
        "-C",
        path_str(&seed),
        "-c",
        "user.name=Seed",
        "-c",
        "user.email=seed@example.test",
        "add",
        "README.md",
    ]);
    git([
        "-C",
        path_str(&seed),
        "-c",
        "user.name=Seed",
        "-c",
        "user.email=seed@example.test",
        "commit",
        "-m",
        "initial",
    ]);
    git([
        "-C",
        path_str(&seed),
        "remote",
        "add",
        "origin",
        path_str(origin),
    ]);
    git(["-C", path_str(&seed), "push", "origin", "main"]);
}

fn git<const N: usize>(args: [&str; N]) {
    let output = Command::new("git").args(args).output().expect("git");
    assert!(
        output.status.success(),
        "git {args:?} failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_output<const N: usize>(args: [&str; N]) -> String {
    let output = Command::new("git").args(args).output().expect("git");
    assert!(
        output.status.success(),
        "git {args:?} failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("utf8")
        .trim_end_matches('\n')
        .to_string()
}

fn path_str(p: &Path) -> &str {
    p.as_os_str().to_str().expect("utf8 path")
}
