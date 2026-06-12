// SPDX-License-Identifier: MPL-2.0

use std::time::Duration;
use std::time::Instant;

use serde_json::json;
use temper_io_engine::http::{HttpResponseData, JsonClient};
use temper_worker_protocol::{
    Artifact, Capability, Capacity, ErrorCode, Heartbeat, JobResult, Poll, Register,
    ReleaseDisposition, ResultStatus, WorkerProtocolMessage, WORKER_PROTOCOL_VERSION,
};

async fn spawn() -> (temper_daemon::Daemon, String) {
    let daemon = temper_daemon::Daemon::new();
    let server = temper_daemon::serve(&daemon, "127.0.0.1:0".parse().expect("loopback addr"))
        .await
        .expect("bind test server");
    (daemon, format!("http://{}/v1/message", server.local_addr()))
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

async fn post(client: &JsonClient, url: &str, msg: &WorkerProtocolMessage) -> HttpResponseData {
    client
        .send(
            "POST",
            url,
            None,
            Some(&serde_json::to_value(msg).expect("message serializes")),
        )
        .await
        .expect("post message")
}

async fn post_json(
    client: &JsonClient,
    url: &str,
    msg: &WorkerProtocolMessage,
) -> WorkerProtocolMessage {
    let response = post(client, url, msg).await;
    assert_eq!(response.status, 200);
    serde_json::from_slice(&response.body).expect("protocol response json")
}

fn assert_error(msg: WorkerProtocolMessage, code: ErrorCode) {
    match msg {
        WorkerProtocolMessage::Error(error) => assert_eq!(error.code, code),
        other => panic!("expected error {code:?}, got {other:?}"),
    }
}

#[test]
fn register_then_poll_returns_assignment_when_matching_work_exists() {
    temper_io_engine::block_on(async move {
        let (daemon, url) = spawn().await;
        let client = JsonClient::new();
        assert_eq!(
            post(
                &client,
                &url,
                &register("worker-a", "engineer", "ai/temper", 1)
            )
            .await
            .status,
            204
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
    })
}

#[test]
fn poll_with_no_work_blocks_then_returns_poll_timeout() {
    temper_io_engine::block_on(async move {
        let (_, url) = spawn().await;
        let client = JsonClient::new();
        assert_eq!(
            post(
                &client,
                &url,
                &register("worker-a", "engineer", "ai/temper", 1)
            )
            .await
            .status,
            204
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
    })
}

#[test]
fn poll_matches_worker_capability_only() {
    temper_io_engine::block_on(async move {
        let (daemon, url) = spawn().await;
        let client = JsonClient::new();
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
    })
}

#[test]
fn enqueue_mid_poll_wakes_and_assigns_promptly() {
    temper_io_engine::block_on(async move {
        let (daemon, url) = spawn().await;
        let client = JsonClient::new();
        let _ = post(
            &client,
            &url,
            &register("worker-a", "engineer", "ai/temper", 1),
        )
        .await;

        let poll_client = client.clone();
        let poll_url = url.clone();
        let poll_task = skein::runtime::Runtime::current_handle()
            .expect("engine runtime")
            .spawn(async move {
                let started = Instant::now();
                let reply =
                    post_json(&poll_client, &poll_url, &poll_with_wait("worker-a", 5_000)).await;
                (reply, started.elapsed())
            });

        temper_io_engine::runtime::sleep_for(Duration::from_millis(200)).await;
        daemon
            .enqueue_job(
                "job-1",
                "engineer",
                "ai/temper",
                artifact(),
                json!({"prompt":"implement"}),
            )
            .await;

        let (reply, elapsed) = poll_task.await;
        match reply {
            WorkerProtocolMessage::Assign(assign) => assert_eq!(assign.job_id, "job-1"),
            other => panic!("expected assign, got {other:?}"),
        }
        assert!(elapsed < Duration::from_secs(3), "elapsed: {elapsed:?}");
    })
}

#[test]
fn saturated_worker_blocks_then_returns_poll_timeout() {
    temper_io_engine::block_on(async move {
        let (daemon, url) = spawn().await;
        let client = JsonClient::new();
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
    })
}
#[test]
fn result_release_frees_capacity() {
    temper_io_engine::block_on(async move {
        let (daemon, url) = spawn().await;
        let client = JsonClient::new();
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
    })
}

#[test]
fn heartbeat_semantics() {
    temper_io_engine::block_on(async move {
        let (_, url) = spawn().await;
        let client = JsonClient::new();
        let _ = post(
            &client,
            &url,
            &register("worker-a", "engineer", "ai/temper", 1),
        )
        .await;
        assert_eq!(
            post(&client, &url, &heartbeat("worker-a")).await.status,
            204
        );
        assert_error(
            post_json(&client, &url, &heartbeat("missing")).await,
            ErrorCode::UnknownWorker,
        );
    })
}

#[test]
fn protocol_version_mismatch() {
    temper_io_engine::block_on(async move {
        let (_, url) = spawn().await;
        let client = JsonClient::new();
        let mut msg = register("worker-a", "engineer", "ai/temper", 1);
        if let WorkerProtocolMessage::Register(register) = &mut msg {
            register.protocol_version = WORKER_PROTOCOL_VERSION + 1;
        }
        assert_error(
            post_json(&client, &url, &msg).await,
            ErrorCode::ProtocolVersionMismatch,
        );
    })
}

#[test]
fn malformed_request_body() {
    temper_io_engine::block_on(async move {
        let (_, url) = spawn().await;
        let client = temper_io_engine::http::build_http_client();
        let cx = temper_io_engine::runtime::current_cx();
        let response = temper_io_engine::http::http_call(
            &cx,
            &client,
            temper_io_engine::http::HttpCall {
                method: "POST".into(),
                url,
                headers: Vec::new(),
                body: b"{ not valid }".to_vec(),
            },
        )
        .await
        .expect("post malformed request");
        assert_eq!(response.status, 400);
    })
}
