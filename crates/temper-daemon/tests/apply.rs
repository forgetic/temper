// SPDX-License-Identifier: MPL-2.0

use std::{
    future::IntoFuture,
    sync::{Arc, Mutex as StdMutex},
};

use axum::http::StatusCode;
use serde_json::json;
use temper_worker_protocol::{
    Artifact, Branch, Capability, Capacity, ErrorCode, Failure, FailureClass, JobResult, Poll,
    Register, ReleaseDisposition, ResultStatus, WorkerProtocolMessage, WORKER_PROTOCOL_VERSION,
};
use temper_worker_registry::InFlightJob;
use tokio::{
    sync::{mpsc, oneshot},
    time::{sleep, Duration},
};

struct RecordingApplier {
    tx: mpsc::UnboundedSender<(InFlightJob, JobResult)>,
}

struct GatedApplier {
    tx: mpsc::UnboundedSender<(InFlightJob, JobResult)>,
    releases: StdMutex<Vec<oneshot::Receiver<()>>>,
}

impl GatedApplier {
    fn new(
        tx: mpsc::UnboundedSender<(InFlightJob, JobResult)>,
        releases: Vec<oneshot::Receiver<()>>,
    ) -> Self {
        Self {
            tx,
            releases: StdMutex::new(releases),
        }
    }
}

#[async_trait::async_trait]
impl temper_daemon::ResultApplier for GatedApplier {
    async fn apply(&self, job: InFlightJob, result: JobResult) {
        let release = {
            let mut releases = self
                .releases
                .lock()
                .expect("release gate lock is not poisoned");
            assert!(
                !releases.is_empty(),
                "test supplies one release gate per apply call"
            );
            releases.remove(0)
        };
        let _ = self.tx.send((job, result));
        let _ = release.await;
    }
}

#[async_trait::async_trait]
impl temper_daemon::ResultApplier for RecordingApplier {
    async fn apply(&self, job: InFlightJob, result: JobResult) {
        let _ = self.tx.send((job, result));
    }
}

async fn spawn_with_applier(
    applier: Arc<dyn temper_daemon::ResultApplier>,
) -> (temper_daemon::Daemon, String) {
    let daemon = temper_daemon::Daemon::with_applier(applier);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let addr = listener.local_addr().expect("read local addr");
    tokio::spawn(axum::serve(listener, daemon.router()).into_future());
    (daemon, format!("http://{addr}/v1/message"))
}

async fn spawn_recording() -> (
    temper_daemon::Daemon,
    String,
    mpsc::UnboundedReceiver<(InFlightJob, JobResult)>,
) {
    let (tx, rx) = mpsc::unbounded_channel();
    let (daemon, url) = spawn_with_applier(Arc::new(RecordingApplier { tx })).await;
    (daemon, url, rx)
}

fn register(worker_id: &str) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Register(Register {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        capabilities: vec![Capability {
            role: "engineer".to_string(),
            repo: "ai/temper".to_string(),
        }],
        capacity: Capacity {
            max_concurrent_jobs: 1,
        },
        labels: None,
    })
}

fn poll_with_wait(worker_id: &str, max_wait_ms: u64) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Poll(Poll {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        free_capacity: 1,
        max_wait_ms: Some(max_wait_ms),
    })
}

fn poll(worker_id: &str) -> WorkerProtocolMessage {
    poll_with_wait(worker_id, 30_000)
}

fn job_result(worker_id: &str, job_id: &str, branch: Option<Branch>) -> JobResult {
    JobResult {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        job_id: job_id.to_string(),
        status: ResultStatus::Success,
        branch,
        verdict: None,
        body: None,
        failure: None,
        summary: Some("done".to_string()),
        details: None,
    }
}

fn transient_failure_result(worker_id: &str, job_id: &str) -> JobResult {
    failure_result(
        worker_id,
        job_id,
        FailureClass::Transient,
        "temporary worker failure",
        "transient failure",
    )
}

fn permanent_failure_result(worker_id: &str, job_id: &str) -> JobResult {
    failure_result(
        worker_id,
        job_id,
        FailureClass::Permanent,
        "worker could not complete the job",
        "permanent failure",
    )
}

fn failure_result(
    worker_id: &str,
    job_id: &str,
    class: FailureClass,
    message: &str,
    summary: &str,
) -> JobResult {
    JobResult {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        job_id: job_id.to_string(),
        status: ResultStatus::Failure,
        branch: None,
        verdict: None,
        body: None,
        failure: Some(Failure {
            class,
            message: message.to_string(),
        }),
        summary: Some(summary.to_string()),
        details: None,
    }
}

fn success_result(worker_id: &str, job_id: &str) -> JobResult {
    job_result(
        worker_id,
        job_id,
        Some(Branch {
            name: "agent/pr-for-code-114".to_string(),
            head_sha: "abc123".to_string(),
        }),
    )
}

fn artifact() -> Artifact {
    Artifact {
        item: json!(114),
        kind: "issue".to_string(),
    }
}

async fn post(
    client: &reqwest::Client,
    url: &str,
    msg: &WorkerProtocolMessage,
) -> reqwest::Response {
    client
        .post(url)
        .json(msg)
        .send()
        .await
        .expect("post message")
}

async fn post_json(
    client: &reqwest::Client,
    url: &str,
    msg: &WorkerProtocolMessage,
) -> WorkerProtocolMessage {
    let response = post(client, url, msg).await;
    assert_eq!(response.status(), StatusCode::OK);
    response.json().await.expect("protocol response json")
}

fn assert_release(msg: WorkerProtocolMessage, worker_id: &str, job_id: &str) {
    match msg {
        WorkerProtocolMessage::Release(release) => {
            assert_eq!(release.worker_id, worker_id);
            assert_eq!(release.job_id, job_id);
            assert_eq!(release.disposition, ReleaseDisposition::Accepted);
        }
        other => panic!("expected release, got {other:?}"),
    }
}

fn assert_assigned(msg: WorkerProtocolMessage, job_id: &str) {
    match msg {
        WorkerProtocolMessage::Assign(assign) => assert_eq!(assign.job_id, job_id),
        other => panic!("expected assign, got {other:?}"),
    }
}

fn assignment_job_id(msg: WorkerProtocolMessage) -> String {
    match msg {
        WorkerProtocolMessage::Assign(assign) => assign.job_id,
        other => panic!("expected assign, got {other:?}"),
    }
}

fn assert_poll_timeout(msg: WorkerProtocolMessage) {
    match msg {
        WorkerProtocolMessage::Error(error) => assert_eq!(error.code, ErrorCode::PollTimeout),
        other => panic!("expected poll timeout, got {other:?}"),
    }
}

async fn enqueue_standard_job(daemon: &temper_daemon::Daemon, job_id: &str) {
    daemon
        .enqueue_job(
            job_id,
            "engineer",
            "ai/temper",
            artifact(),
            json!({"prompt":"implement", "issue":114}),
        )
        .await;
}

async fn eventually_enqueue_and_assign(
    daemon: &temper_daemon::Daemon,
    client: &reqwest::Client,
    url: &str,
    worker_id: &str,
    job_id: &str,
) {
    for _ in 0..20 {
        enqueue_standard_job(daemon, job_id).await;
        match post_json(client, url, &poll_with_wait(worker_id, 25)).await {
            WorkerProtocolMessage::Assign(assign) if assign.job_id == job_id => return,
            WorkerProtocolMessage::Error(error) if error.code == ErrorCode::PollTimeout => {
                sleep(Duration::from_millis(10)).await;
            }
            other => panic!("expected assign for {job_id} or poll timeout, got {other:?}"),
        }
    }

    panic!("job {job_id} did not become dispatchable after apply finished");
}

#[tokio::test]
async fn accepted_result_invokes_applier_with_in_flight_context() {
    let (daemon, url, mut rx) = spawn_recording().await;
    let client = reqwest::Client::new();
    assert_eq!(
        post(&client, &url, &register("worker-a")).await.status(),
        StatusCode::NO_CONTENT
    );

    let artifact = artifact();
    let payload = json!({"prompt":"implement", "issue":114});
    daemon
        .enqueue_job(
            "job-apply-1",
            "engineer",
            "ai/temper",
            artifact.clone(),
            payload.clone(),
        )
        .await;

    assert_assigned(
        post_json(&client, &url, &poll("worker-a")).await,
        "job-apply-1",
    );

    let branch = Branch {
        name: "agent/pr-for-code-114".to_string(),
        head_sha: "abc123".to_string(),
    };
    let posted_result = job_result("worker-a", "job-apply-1", Some(branch));
    assert_release(
        post_json(
            &client,
            &url,
            &WorkerProtocolMessage::Result(posted_result.clone()),
        )
        .await,
        "worker-a",
        "job-apply-1",
    );

    let (job, recorded_result) = rx.recv().await.expect("applier records accepted result");
    assert_eq!(job.job_id, "job-apply-1");
    assert_eq!(job.role, "engineer");
    assert_eq!(job.repo, "ai/temper");
    assert_eq!(job.artifact, artifact);
    assert_eq!(job.job_payload, payload);
    assert_eq!(recorded_result.job_id, posted_result.job_id);
    assert_eq!(recorded_result.status, posted_result.status);
    assert_eq!(recorded_result.branch, posted_result.branch);
    assert!(matches!(
        rx.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn result_without_in_flight_job_does_not_invoke_applier() {
    let (daemon, url, mut rx) = spawn_recording().await;
    let client = reqwest::Client::new();
    assert_eq!(
        post(&client, &url, &register("worker-a")).await.status(),
        StatusCode::NO_CONTENT
    );

    assert_release(
        post_json(
            &client,
            &url,
            &WorkerProtocolMessage::Result(job_result("worker-a", "phantom-job", None)),
        )
        .await,
        "worker-a",
        "phantom-job",
    );

    daemon
        .enqueue_job(
            "pending-job",
            "architect",
            "ai/temper",
            artifact(),
            json!({"n":"pending"}),
        )
        .await;
    assert_release(
        post_json(
            &client,
            &url,
            &WorkerProtocolMessage::Result(job_result("worker-a", "pending-job", None)),
        )
        .await,
        "worker-a",
        "pending-job",
    );

    daemon
        .enqueue_job(
            "real-job",
            "engineer",
            "ai/temper",
            artifact(),
            json!({"n":1}),
        )
        .await;
    assert_assigned(
        post_json(&client, &url, &poll("worker-a")).await,
        "real-job",
    );

    let real_result = job_result("worker-a", "real-job", None);
    assert_release(
        post_json(
            &client,
            &url,
            &WorkerProtocolMessage::Result(real_result.clone()),
        )
        .await,
        "worker-a",
        "real-job",
    );

    let (job, recorded_result) = rx.recv().await.expect("applier records real result");
    assert_eq!(job.job_id, "real-job");
    assert_eq!(recorded_result.job_id, real_result.job_id);
    assert!(matches!(
        rx.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn apply_window_blocks_duplicate_enqueue_until_apply_finishes() {
    let (record_tx, mut rx) = mpsc::unbounded_channel();
    let (release_tx, release_rx) = oneshot::channel();
    let (daemon, url) =
        spawn_with_applier(Arc::new(GatedApplier::new(record_tx, vec![release_rx]))).await;
    let client = reqwest::Client::new();
    assert_eq!(
        post(&client, &url, &register("worker-a")).await.status(),
        StatusCode::NO_CONTENT
    );

    enqueue_standard_job(&daemon, "job-apply-window-1").await;
    assert_assigned(
        post_json(&client, &url, &poll("worker-a")).await,
        "job-apply-window-1",
    );

    let result = success_result("worker-a", "job-apply-window-1");
    assert_release(
        post_json(
            &client,
            &url,
            &WorkerProtocolMessage::Result(result.clone()),
        )
        .await,
        "worker-a",
        "job-apply-window-1",
    );
    let (job, recorded_result) = rx.recv().await.expect("applier starts and parks");
    assert_eq!(job.job_id, "job-apply-window-1");
    assert_eq!(recorded_result.job_id, result.job_id);

    enqueue_standard_job(&daemon, "job-apply-window-1").await;
    assert_poll_timeout(post_json(&client, &url, &poll_with_wait("worker-a", 25)).await);

    release_tx.send(()).expect("release apply gate");
    eventually_enqueue_and_assign(&daemon, &client, &url, "worker-a", "job-apply-window-1").await;
}

#[tokio::test]
async fn transient_failure_drops_job_for_rescan() {
    let (daemon, url, mut rx) = spawn_recording().await;
    let client = reqwest::Client::new();
    assert_eq!(
        post(&client, &url, &register("worker-a")).await.status(),
        StatusCode::NO_CONTENT
    );

    enqueue_standard_job(&daemon, "job-retry-1").await;

    let first_job_id = assignment_job_id(post_json(&client, &url, &poll("worker-a")).await);
    assert_eq!(first_job_id, "job-retry-1");

    let transient = transient_failure_result("worker-a", &first_job_id);
    assert_release(
        post_json(
            &client,
            &url,
            &WorkerProtocolMessage::Result(transient.clone()),
        )
        .await,
        "worker-a",
        &first_job_id,
    );

    assert_poll_timeout(post_json(&client, &url, &poll_with_wait("worker-a", 25)).await);
    assert!(matches!(
        rx.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));

    enqueue_standard_job(&daemon, &first_job_id).await;
    let retry_job_id = assignment_job_id(post_json(&client, &url, &poll("worker-a")).await);
    assert_eq!(retry_job_id, first_job_id);

    let final_result = success_result("worker-a", &retry_job_id);
    assert_release(
        post_json(
            &client,
            &url,
            &WorkerProtocolMessage::Result(final_result.clone()),
        )
        .await,
        "worker-a",
        &retry_job_id,
    );

    let (job, recorded_result) = rx
        .recv()
        .await
        .expect("applier records final success result");
    assert_eq!(job.job_id, retry_job_id);
    assert_eq!(recorded_result.job_id, final_result.job_id);
    assert_eq!(recorded_result.status, ResultStatus::Success);
    assert_eq!(recorded_result.branch, final_result.branch);
    assert!(matches!(
        rx.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn permanent_failure_apply_window_unblocks_after_apply_completes() {
    let (record_tx, mut rx) = mpsc::unbounded_channel();
    let (release_tx, release_rx) = oneshot::channel();
    let (daemon, url) =
        spawn_with_applier(Arc::new(GatedApplier::new(record_tx, vec![release_rx]))).await;
    let client = reqwest::Client::new();
    assert_eq!(
        post(&client, &url, &register("worker-a")).await.status(),
        StatusCode::NO_CONTENT
    );

    enqueue_standard_job(&daemon, "job-permanent-failure-1").await;
    assert_assigned(
        post_json(&client, &url, &poll("worker-a")).await,
        "job-permanent-failure-1",
    );

    let failure = permanent_failure_result("worker-a", "job-permanent-failure-1");
    assert_release(
        post_json(
            &client,
            &url,
            &WorkerProtocolMessage::Result(failure.clone()),
        )
        .await,
        "worker-a",
        "job-permanent-failure-1",
    );
    let (job, recorded_result) = rx.recv().await.expect("applier starts and parks");
    assert_eq!(job.job_id, "job-permanent-failure-1");
    assert_eq!(recorded_result.job_id, failure.job_id);
    assert_eq!(recorded_result.status, ResultStatus::Failure);
    assert_eq!(
        recorded_result
            .failure
            .as_ref()
            .map(|failure| failure.class),
        Some(FailureClass::Permanent)
    );

    enqueue_standard_job(&daemon, "job-permanent-failure-1").await;
    assert_poll_timeout(post_json(&client, &url, &poll_with_wait("worker-a", 25)).await);

    release_tx.send(()).expect("release apply gate");
    eventually_enqueue_and_assign(
        &daemon,
        &client,
        &url,
        "worker-a",
        "job-permanent-failure-1",
    )
    .await;
}
