// SPDX-License-Identifier: MPL-2.0

use std::{future::IntoFuture, sync::Arc};

use axum::http::StatusCode;
use serde_json::json;
use temper_worker_protocol::{
    Artifact, Branch, Capability, Capacity, JobResult, Poll, Register, ReleaseDisposition,
    ResultStatus, WorkerProtocolMessage, WORKER_PROTOCOL_VERSION,
};
use temper_worker_registry::InFlightJob;
use tokio::sync::mpsc;

struct RecordingApplier {
    tx: mpsc::UnboundedSender<(InFlightJob, JobResult)>,
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

fn poll(worker_id: &str) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Poll(Poll {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        free_capacity: 1,
        max_wait_ms: Some(30_000),
    })
}

fn job_result(worker_id: &str, job_id: &str, branch: Option<Branch>) -> JobResult {
    JobResult {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        job_id: job_id.to_string(),
        status: ResultStatus::Success,
        branch,
        failure: None,
        summary: Some("done".to_string()),
        details: None,
    }
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
