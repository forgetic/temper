// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;

use serde_json::json;
use temper_engine::{
    Daemon, RoleFeedMode, RoleFeedTarget, WebhookConfig, WebhookError, handle_webhook,
    webhook_signature,
};
use temper_forge_model::{CreateIssue, CreateRepository, Forge, ItemNumber, RepositoryId};
use temper_forge_memory::MemoryForge;
use temper_worker_protocol::{
    Capability, Capacity, ErrorCode, Poll, Register, WORKER_PROTOCOL_VERSION, WorkerProtocolMessage,
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
    let server = temper_engine::serve(
        handle,
        &daemon,
        "127.0.0.1:0".parse().expect("loopback addr"),
    )
    .await
    .expect("bind test server");
    let addr = server.local_addr();
    (daemon, format!("http://{addr}/v1/message"))
}

async fn create_repo(forge: &MemoryForge, owner: &str, name: &str) -> RepositoryId {
    forge
        .create_repository(CreateRepository {
            owner: owner.to_string(),
            name: name.to_string(),
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
    client: &temper_engine_io::http::JsonClient,
    url: &str,
    msg: &WorkerProtocolMessage,
) -> temper_engine_io::http::HttpResponseData {
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
    client: &temper_engine_io::http::JsonClient,
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

fn signed_headers(secret: &str, body: &[u8], event: &str) -> BTreeMap<String, String> {
    let mut headers = BTreeMap::new();
    headers.insert("x-forgejo-event".to_string(), event.to_string());
    headers.insert(
        "x-forgejo-signature".to_string(),
        format!("sha256={}", webhook_signature(secret, body)),
    );
    headers
}

fn assert_scanned_issue_assignment(msg: WorkerProtocolMessage, issue: ItemNumber) {
    match msg {
        WorkerProtocolMessage::Assign(assign) => {
            assert_eq!(assign.repo, "acme/service");
            assert_eq!(assign.role, "engineer");
            assert_eq!(assign.artifact.kind, "issue");
            assert_eq!(assign.artifact.item, json!(issue.get()));
            assert!(
                assign
                    .job_id
                    .contains(&format!("/issue-{}/engineer/", issue.get()))
            );
        }
        other => panic!("expected assign, got {other:?}"),
    }
}

fn webhook_config(repo: RepositoryId) -> WebhookConfig {
    WebhookConfig {
        secret: "s3cret".into(),
        targets: vec![RoleFeedTarget {
            repo,
            role: RoleId::new("engineer"),
            mode: RoleFeedMode::Wake,
        }],
    }
}

#[test]
fn verified_webhook_wakes_matching_target_then_dispatches() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let forge = MemoryForge::new();
        let repo = create_repo(&forge, "acme", "service").await;
        let issue = create_issue(&forge, &repo, &["code", "ready"]).await;
        let workflow = workflow();
        let compiled = workflow.compile();
        let (daemon, url) = spawn(&handle).await;
        let client = temper_engine_io::http::JsonClient::new();
        let config = webhook_config(repo.clone());
        let body = serde_json::to_vec(&json!({
            "repository": { "full_name": "acme/service" },
            "issue": { "number": issue.get() }
        }))
        .expect("webhook body serializes");
        let headers = signed_headers(&config.secret, &body, "issues");

        assert_eq!(
            handle_webhook(
                &daemon,
                &forge,
                &workflow,
                &compiled,
                ts("2026-05-29T00:00:00Z"),
                &config,
                &headers,
                &body,
            )
            .await,
            Ok(1)
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
fn webhook_with_invalid_signature_is_rejected_and_enqueues_nothing() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let forge = MemoryForge::new();
        let repo = create_repo(&forge, "acme", "service").await;
        let issue = create_issue(&forge, &repo, &["code", "ready"]).await;
        let workflow = workflow();
        let compiled = workflow.compile();
        let (daemon, url) = spawn(&handle).await;
        let client = temper_engine_io::http::JsonClient::new();
        let config = webhook_config(repo);
        let body = serde_json::to_vec(&json!({
            "repository": { "full_name": "acme/service" },
            "issue": { "number": issue.get() }
        }))
        .expect("webhook body serializes");
        let mut headers = BTreeMap::new();
        headers.insert("x-forgejo-event".to_string(), "issues".to_string());
        headers.insert("x-forgejo-signature".to_string(), "sha256=00".to_string());

        assert_eq!(
            handle_webhook(
                &daemon,
                &forge,
                &workflow,
                &compiled,
                ts("2026-05-29T00:00:00Z"),
                &config,
                &headers,
                &body,
            )
            .await,
            Err(WebhookError::InvalidSignature)
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
fn webhook_for_unconfigured_repo_enqueues_nothing() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let forge = MemoryForge::new();
        let configured_repo = create_repo(&forge, "acme", "service").await;
        let other_repo = create_repo(&forge, "acme", "other").await;
        create_issue(&forge, &configured_repo, &["code", "ready"]).await;
        let issue = create_issue(&forge, &other_repo, &["code", "ready"]).await;
        let workflow = workflow();
        let compiled = workflow.compile();
        let (daemon, url) = spawn(&handle).await;
        let client = temper_engine_io::http::JsonClient::new();
        let config = webhook_config(configured_repo);
        let body = serde_json::to_vec(&json!({
            "repository": { "full_name": "acme/other" },
            "issue": { "number": issue.get() }
        }))
        .expect("webhook body serializes");
        let headers = signed_headers(&config.secret, &body, "issues");

        assert_eq!(
            handle_webhook(
                &daemon,
                &forge,
                &workflow,
                &compiled,
                ts("2026-05-29T00:00:00Z"),
                &config,
                &headers,
                &body,
            )
            .await,
            Ok(0)
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
