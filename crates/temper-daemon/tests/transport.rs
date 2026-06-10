// SPDX-License-Identifier: MPL-2.0

use std::time::Instant;
use std::{future::IntoFuture, time::Duration};

use axum::http::StatusCode;
use serde_json::json;
use temper_worker_protocol::{
    Artifact, Capability, Capacity, ErrorCode, Heartbeat, JobResult, Poll, Register,
    ReleaseDisposition, ResultStatus, WorkerProtocolMessage, WORKER_PROTOCOL_VERSION,
};

async fn spawn() -> (temper_daemon::Daemon, String) {
    let daemon = temper_daemon::Daemon::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let addr = listener.local_addr().expect("read local addr");
    tokio::spawn(axum::serve(listener, daemon.router()).into_future());
    (daemon, format!("http://{addr}/v1/message"))
}

fn register(
    worker_id: &str,
    role: &str,
    repo: &str,
    max_concurrent_jobs: u32,
) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Register(Register {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        capabilities: vec![Capability {
            role: role.to_string(),
            repo: repo.to_string(),
        }],
        capacity: Capacity {
            max_concurrent_jobs,
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

fn heartbeat(worker_id: &str) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Heartbeat(Heartbeat {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        jobs: Vec::new(),
        free_capacity: Some(1),
    })
}

fn result(worker_id: &str, job_id: &str) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Result(JobResult {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        job_id: job_id.to_string(),
        status: ResultStatus::Success,
        branch: None,
        verdict: None,
        body: None,
        failure: None,
        summary: Some("done".to_string()),
        details: None,
    })
}

fn artifact() -> Artifact {
    Artifact {
        item: json!(103),
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

fn assert_error(msg: WorkerProtocolMessage, code: ErrorCode) {
    match msg {
        WorkerProtocolMessage::Error(error) => assert_eq!(error.code, code),
        other => panic!("expected error {code:?}, got {other:?}"),
    }
}

#[tokio::test]
async fn register_then_poll_returns_assignment_when_matching_work_exists() {
    let (daemon, url) = spawn().await;
    let client = reqwest::Client::new();
    assert_eq!(
        post(
            &client,
            &url,
            &register("worker-a", "engineer", "ai/temper", 1)
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    let artifact = artifact();
    let payload = json!({"prompt":"implement"});
    daemon
        .enqueue_job(
            "job-1",
            "engineer",
            "ai/temper",
            artifact.clone(),
            payload.clone(),
        )
        .await;

    match post_json(&client, &url, &poll("worker-a")).await {
        WorkerProtocolMessage::Assign(assign) => {
            assert_eq!(assign.job_id, "job-1");
            assert_eq!(assign.role, "engineer");
            assert_eq!(assign.repo, "ai/temper");
            assert_eq!(assign.artifact, artifact);
            assert_eq!(assign.job_payload, payload);
        }
        other => panic!("expected assign, got {other:?}"),
    }
}

#[tokio::test]
async fn poll_with_no_work_blocks_then_returns_poll_timeout() {
    let (_, url) = spawn().await;
    let client = reqwest::Client::new();
    assert_eq!(
        post(
            &client,
            &url,
            &register("worker-a", "engineer", "ai/temper", 1)
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );

    let started = Instant::now();
    assert_error(
        post_json(&client, &url, &poll_with_wait("worker-a", 300)).await,
        ErrorCode::PollTimeout,
    );
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(250),
        "elapsed: {elapsed:?}"
    );
    assert!(elapsed < Duration::from_secs(5), "elapsed: {elapsed:?}");
}

#[tokio::test]
async fn poll_matches_worker_capability_only() {
    let (daemon, url) = spawn().await;
    let client = reqwest::Client::new();
    daemon
        .enqueue_job("job-1", "architect", "ai/temper", artifact(), json!({}))
        .await;
    let _ = post(
        &client,
        &url,
        &register("engineer-a", "engineer", "ai/temper", 1),
    )
    .await;
    assert_error(
        post_json(&client, &url, &poll_with_wait("engineer-a", 300)).await,
        ErrorCode::PollTimeout,
    );
    let _ = post(
        &client,
        &url,
        &register("architect-a", "architect", "ai/temper", 1),
    )
    .await;
    match post_json(&client, &url, &poll("architect-a")).await {
        WorkerProtocolMessage::Assign(assign) => assert_eq!(assign.job_id, "job-1"),
        other => panic!("expected assign, got {other:?}"),
    }
}

#[tokio::test]
async fn enqueue_mid_poll_wakes_and_assigns_promptly() {
    let (daemon, url) = spawn().await;
    let client = reqwest::Client::new();
    let _ = post(
        &client,
        &url,
        &register("worker-a", "engineer", "ai/temper", 1),
    )
    .await;

    let poll_client = client.clone();
    let poll_url = url.clone();
    let poll_task = tokio::spawn(async move {
        let started = Instant::now();
        let reply = post_json(&poll_client, &poll_url, &poll_with_wait("worker-a", 5_000)).await;
        (reply, started.elapsed())
    });

    tokio::time::sleep(Duration::from_millis(200)).await;
    daemon
        .enqueue_job(
            "job-1",
            "engineer",
            "ai/temper",
            artifact(),
            json!({"prompt":"implement"}),
        )
        .await;

    let (reply, elapsed) = poll_task.await.expect("poll task completes");
    match reply {
        WorkerProtocolMessage::Assign(assign) => assert_eq!(assign.job_id, "job-1"),
        other => panic!("expected assign, got {other:?}"),
    }
    assert!(elapsed < Duration::from_secs(3), "elapsed: {elapsed:?}");
}

#[tokio::test]
async fn saturated_worker_blocks_then_returns_poll_timeout() {
    let (daemon, url) = spawn().await;
    let client = reqwest::Client::new();
    let _ = post(
        &client,
        &url,
        &register("worker-a", "engineer", "ai/temper", 1),
    )
    .await;
    daemon
        .enqueue_job("job-1", "engineer", "ai/temper", artifact(), json!({}))
        .await;

    match post_json(&client, &url, &poll("worker-a")).await {
        WorkerProtocolMessage::Assign(assign) => assert_eq!(assign.job_id, "job-1"),
        other => panic!("expected assign, got {other:?}"),
    }

    let started = Instant::now();
    assert_error(
        post_json(&client, &url, &poll_with_wait("worker-a", 300)).await,
        ErrorCode::PollTimeout,
    );
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(250),
        "elapsed: {elapsed:?}"
    );
    assert!(elapsed < Duration::from_secs(5), "elapsed: {elapsed:?}");
}
#[tokio::test]
async fn result_release_frees_capacity() {
    let (daemon, url) = spawn().await;
    let client = reqwest::Client::new();
    let _ = post(
        &client,
        &url,
        &register("worker-a", "engineer", "ai/temper", 1),
    )
    .await;
    daemon
        .enqueue_job("job-1", "engineer", "ai/temper", artifact(), json!({"n":1}))
        .await;
    daemon
        .enqueue_job("job-2", "engineer", "ai/temper", artifact(), json!({"n":2}))
        .await;
    match post_json(&client, &url, &poll("worker-a")).await {
        WorkerProtocolMessage::Assign(assign) => assert_eq!(assign.job_id, "job-1"),
        other => panic!("expected assign, got {other:?}"),
    }
    match post_json(&client, &url, &result("worker-a", "job-1")).await {
        WorkerProtocolMessage::Release(release) => {
            assert_eq!(release.disposition, ReleaseDisposition::Accepted)
        }
        other => panic!("expected release, got {other:?}"),
    }
    match post_json(&client, &url, &poll("worker-a")).await {
        WorkerProtocolMessage::Assign(assign) => assert_eq!(assign.job_id, "job-2"),
        other => panic!("expected assign, got {other:?}"),
    }
}

#[tokio::test]
async fn heartbeat_semantics() {
    let (_, url) = spawn().await;
    let client = reqwest::Client::new();
    let _ = post(
        &client,
        &url,
        &register("worker-a", "engineer", "ai/temper", 1),
    )
    .await;
    assert_eq!(
        post(&client, &url, &heartbeat("worker-a")).await.status(),
        StatusCode::NO_CONTENT
    );
    assert_error(
        post_json(&client, &url, &heartbeat("missing")).await,
        ErrorCode::UnknownWorker,
    );
}

#[tokio::test]
async fn protocol_version_mismatch() {
    let (_, url) = spawn().await;
    let client = reqwest::Client::new();
    let mut msg = register("worker-a", "engineer", "ai/temper", 1);
    if let WorkerProtocolMessage::Register(register) = &mut msg {
        register.protocol_version = WORKER_PROTOCOL_VERSION + 1;
    }
    assert_error(
        post_json(&client, &url, &msg).await,
        ErrorCode::ProtocolVersionMismatch,
    );
}

#[tokio::test]
async fn malformed_request_body() {
    let (_, url) = spawn().await;
    let response = reqwest::Client::new()
        .post(url)
        .body("{ not valid }")
        .send()
        .await
        .expect("post malformed request");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
