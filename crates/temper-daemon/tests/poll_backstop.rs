// SPDX-License-Identifier: MPL-2.0

use std::{future::IntoFuture, time::Duration};

use axum::http::StatusCode;
use serde_json::json;
use temper_daemon::{
    run_poll_backstop_tick, Daemon, PollBackstopConfig, RoleFeedMode, RoleFeedTarget,
};
use temper_forge::{CreateIssue, CreateRepository, Forge, ItemNumber, RepositoryId};
use temper_forge_memory::MemoryForge;
use temper_worker_protocol::{
    Capability, Capacity, ErrorCode, Poll, Register, WorkerProtocolMessage, WORKER_PROTOCOL_VERSION,
};
use temper_workflow::{RawWorkflowSpec, RoleId, ValidatedWorkflow};

const FIXTURE: &str = include_str!("../../temper-workflow/fixtures/reference-delivery.json");

fn workflow() -> ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(FIXTURE).expect("workflow parses");
    spec.validate().expect("workflow validates")
}

fn ts(value: &str) -> chrono::DateTime<chrono::Utc> {
    value.parse().expect("valid RFC 3339 timestamp")
}

async fn spawn() -> (Daemon, String) {
    let daemon = Daemon::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let addr = listener.local_addr().expect("read local addr");
    tokio::spawn(axum::serve(listener, daemon.router()).into_future());
    (daemon, format!("http://{addr}/v1/message"))
}

async fn new_repo(forge: &MemoryForge) -> RepositoryId {
    forge
        .create_repository(CreateRepository {
            owner: "acme".to_string(),
            name: "service".to_string(),
            default_branch: "main".to_string(),
            description: None,
        })
        .await
        .expect("repository is created")
        .id
}

async fn create_issue(forge: &MemoryForge, repo: &RepositoryId, labels: &[&str]) -> ItemNumber {
    forge
        .create_issue(
            repo,
            CreateIssue {
                title: "ready code issue".to_string(),
                body: String::new(),
                labels: labels.iter().map(|label| (*label).to_string()).collect(),
                assignees: Vec::new(),
            },
        )
        .await
        .expect("issue is created")
        .number
}

fn register(worker_id: &str, role: &str, repo: &str) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Register(Register {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        capabilities: vec![Capability {
            role: role.to_string(),
            repo: repo.to_string(),
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

fn assert_poll_timeout(msg: WorkerProtocolMessage) {
    match msg {
        WorkerProtocolMessage::Error(error) => assert_eq!(error.code, ErrorCode::PollTimeout),
        other => panic!("expected poll timeout, got {other:?}"),
    }
}

fn assert_scanned_issue_assignment(msg: WorkerProtocolMessage, issue: ItemNumber) {
    match msg {
        WorkerProtocolMessage::Assign(assign) => {
            assert_eq!(assign.repo, "acme/service");
            assert_eq!(assign.role, "engineer");
            assert_eq!(assign.artifact.kind, "issue");
            assert_eq!(assign.artifact.item, json!(issue.get()));
            assert!(assign
                .job_id
                .contains(&format!("/issue-{}/engineer/", issue.get())));
        }
        other => panic!("expected assign, got {other:?}"),
    }
}

#[tokio::test]
async fn run_poll_backstop_tick_enqueues_scanned_work_then_dispatches() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge).await;
    let issue = create_issue(&forge, &repo, &["code", "ready"]).await;
    let workflow = workflow();
    let compiled = workflow.compile();
    let (daemon, url) = spawn().await;
    let client = reqwest::Client::new();
    let config = PollBackstopConfig {
        targets: vec![RoleFeedTarget {
            repo: repo.clone(),
            role: RoleId::new("engineer"),
            mode: RoleFeedMode::Normal,
        }],
        cadence: Duration::from_millis(10),
    };

    assert_eq!(
        run_poll_backstop_tick(
            &daemon,
            &forge,
            &workflow,
            &compiled,
            ts("2026-05-29T00:00:00Z"),
            &config,
        )
        .await,
        1
    );

    assert_eq!(
        post(
            &client,
            &url,
            &register("worker-a", "engineer", "acme/service")
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );

    assert_scanned_issue_assignment(post_json(&client, &url, &poll("worker-a")).await, issue);
}

#[tokio::test]
async fn run_poll_backstop_tick_with_no_targets_is_zero() {
    let forge = MemoryForge::new();
    let workflow = workflow();
    let compiled = workflow.compile();
    let (daemon, url) = spawn().await;
    let client = reqwest::Client::new();
    let config = PollBackstopConfig {
        targets: vec![],
        cadence: Duration::from_millis(10),
    };

    assert_eq!(
        run_poll_backstop_tick(
            &daemon,
            &forge,
            &workflow,
            &compiled,
            ts("2026-05-29T00:00:00Z"),
            &config,
        )
        .await,
        0
    );

    assert_eq!(
        post(
            &client,
            &url,
            &register("worker-a", "engineer", "acme/service")
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_poll_timeout(post_json(&client, &url, &poll_with_wait("worker-a", 100)).await);
}

#[tokio::test]
async fn run_poll_backstop_tick_skips_failing_target_and_continues() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge).await;
    let issue = create_issue(&forge, &repo, &["code", "ready"]).await;
    let workflow = workflow();
    let compiled = workflow.compile();
    let (daemon, url) = spawn().await;
    let client = reqwest::Client::new();
    let config = PollBackstopConfig {
        targets: vec![
            RoleFeedTarget {
                repo: RepositoryId::new("missing-repo"),
                role: RoleId::new("engineer"),
                mode: RoleFeedMode::Normal,
            },
            RoleFeedTarget {
                repo: repo.clone(),
                role: RoleId::new("engineer"),
                mode: RoleFeedMode::Normal,
            },
        ],
        cadence: Duration::from_millis(10),
    };

    assert_eq!(
        run_poll_backstop_tick(
            &daemon,
            &forge,
            &workflow,
            &compiled,
            ts("2026-05-29T00:00:00Z"),
            &config,
        )
        .await,
        1
    );

    assert_eq!(
        post(
            &client,
            &url,
            &register("worker-a", "engineer", "acme/service")
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_scanned_issue_assignment(post_json(&client, &url, &poll("worker-a")).await, issue);
}
