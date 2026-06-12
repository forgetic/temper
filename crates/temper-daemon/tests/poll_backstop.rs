// SPDX-License-Identifier: MPL-2.0

use std::time::Duration;

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

async fn spawn(handle: &skein::runtime::RuntimeHandle) -> (Daemon, String) {
    let daemon = Daemon::new(std::sync::Arc::new(handle.clone()));
    let server = temper_daemon::serve(
        handle,
        &daemon,
        "127.0.0.1:0".parse().expect("loopback addr"),
    )
    .await
    .expect("bind test server");
    let addr = server.local_addr();
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
    client: &temper_io_engine::http::JsonClient,
    url: &str,
    msg: &WorkerProtocolMessage,
) -> temper_io_engine::http::HttpResponseData {
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
    client: &temper_io_engine::http::JsonClient,
    url: &str,
    msg: &WorkerProtocolMessage,
) -> WorkerProtocolMessage {
    let response = post(client, url, msg).await;
    assert_eq!(response.status, 200);
    serde_json::from_slice(&response.body).expect("protocol response json")
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

#[test]
fn run_poll_backstop_tick_enqueues_scanned_work_then_dispatches() {
    temper_io_engine::block_on_with(move |_cx, handle| async move {
        let forge = MemoryForge::new();
        let repo = new_repo(&forge).await;
        let issue = create_issue(&forge, &repo, &["code", "ready"]).await;
        let workflow = workflow();
        let compiled = workflow.compile();
        let (daemon, url) = spawn(&handle).await;
        let client = temper_io_engine::http::JsonClient::new();
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
            .status,
            204
        );

        assert_scanned_issue_assignment(post_json(&client, &url, &poll("worker-a")).await, issue);
    })
}

#[test]
fn run_poll_backstop_tick_with_no_targets_is_zero() {
    temper_io_engine::block_on_with(move |_cx, handle| async move {
        let forge = MemoryForge::new();
        let workflow = workflow();
        let compiled = workflow.compile();
        let (daemon, url) = spawn(&handle).await;
        let client = temper_io_engine::http::JsonClient::new();
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
            .status,
            204
        );
        assert_poll_timeout(post_json(&client, &url, &poll_with_wait("worker-a", 100)).await);
    })
}

#[test]
fn run_poll_backstop_tick_skips_failing_target_and_continues() {
    temper_io_engine::block_on_with(move |_cx, handle| async move {
        let forge = MemoryForge::new();
        let repo = new_repo(&forge).await;
        let issue = create_issue(&forge, &repo, &["code", "ready"]).await;
        let workflow = workflow();
        let compiled = workflow.compile();
        let (daemon, url) = spawn(&handle).await;
        let client = temper_io_engine::http::JsonClient::new();
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
            .status,
            204
        );
        assert_scanned_issue_assignment(post_json(&client, &url, &poll("worker-a")).await, issue);
    })
}
