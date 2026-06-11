// SPDX-License-Identifier: MPL-2.0

use std::{sync::Arc, time::Duration};

use serde_json::json;
use std::time::Instant;
use temper_daemon::{
    webhook_signature, Daemon, ForgeApplier, LeaseApplier, RoleFeedMode, RoleFeedTarget,
    WebhookConfig,
};
use temper_forge::{
    CreateIssue, CreateRepository, Forge, ItemNumber, PullRequest, PullRequestQuery, RepositoryId,
    UserId,
};
use temper_forge_memory::MemoryForge;
use temper_worker_protocol::{
    Assign, Branch, Capability, Capacity, ErrorCode, JobResult, Poll, Register, ReleaseDisposition,
    ResultStatus, WorkerProtocolMessage, WORKER_PROTOCOL_VERSION,
};
use temper_workflow::{
    parse_metadata_block, ArtifactKindId, ArtifactRef, CompiledWorkflow, LeasePolicy,
    RawWorkflowSpec, RoleId, ValidatedWorkflow,
};

const FIXTURE: &str = include_str!("../../temper-workflow/fixtures/reference-delivery.json");

fn workflow() -> ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(FIXTURE).expect("workflow parses");
    spec.validate().expect("workflow validates")
}

async fn create_repo(
    forge: &MemoryForge,
    owner: &str,
    name: &str,
    default_branch: &str,
) -> RepositoryId {
    forge
        .create_repository(CreateRepository {
            owner: owner.to_string(),
            name: name.to_string(),
            default_branch: default_branch.to_string(),
            description: None,
        })
        .await
        .expect("repository is created")
        .id
}

async fn create_ready_issue(forge: &MemoryForge, repo: &RepositoryId) -> ItemNumber {
    forge
        .create_issue(
            repo,
            CreateIssue {
                title: "ready code issue".to_string(),
                body: "Implement the daemon webhook route.".to_string(),
                labels: vec!["code".to_string(), "ready".to_string()],
                assignees: Vec::<UserId>::new(),
            },
        )
        .await
        .expect("issue is created")
        .number
}

async fn spawn_with_webhook(
    daemon: &Daemon,
    forge: Arc<MemoryForge>,
    workflow: Arc<ValidatedWorkflow>,
    compiled: Arc<CompiledWorkflow>,
    config: Arc<WebhookConfig>,
) -> (String, String) {
    let daemon = daemon
        .clone()
        .with_webhook(forge, workflow, compiled, config);
    let server = temper_daemon::serve(&daemon, "127.0.0.1:0".parse().expect("loopback addr"))
        .await
        .expect("bind test server");
    let addr = server.local_addr();
    (
        format!("http://{addr}/v1/message"),
        format!("http://{addr}/forgejo/webhook"),
    )
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

fn webhook_body(issue: ItemNumber) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "repository": { "full_name": "acme/service" },
        "issue": { "number": issue.get() }
    }))
    .expect("webhook body serializes")
}

async fn post_webhook(
    client: &temper_io_engine::http::JsonClient,
    url: &str,
    secret: &str,
    body: Vec<u8>,
) -> temper_io_engine::http::HttpResponseData {
    let signature = webhook_signature(secret, &body);
    post_webhook_with_signature(client, url, &format!("sha256={signature}"), body).await
}

async fn post_webhook_with_signature(
    _client: &temper_io_engine::http::JsonClient,
    url: &str,
    signature: &str,
    body: Vec<u8>,
) -> temper_io_engine::http::HttpResponseData {
    let pooled = temper_io_engine::http::build_http_client();
    let cx = temper_io_engine::runtime::ambient_cx();
    temper_io_engine::http::http_call(
        &cx,
        &pooled,
        temper_io_engine::http::HttpCall {
            method: "POST".into(),
            url: url.to_string(),
            headers: vec![
                ("x-forgejo-event".to_string(), "issues".to_string()),
                ("x-forgejo-signature".to_string(), signature.to_string()),
            ],
            body,
        },
    )
    .await
    .expect("post webhook")
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

fn success_result(worker_id: &str, job_id: &str, branch_name: &str, summary: &str) -> JobResult {
    JobResult {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        job_id: job_id.to_string(),
        status: ResultStatus::Success,
        branch: Some(Branch {
            name: branch_name.to_string(),
            head_sha: "abc123".to_string(),
        }),
        verdict: None,
        body: None,
        failure: None,
        summary: Some(summary.to_string()),
        details: Some(json!({"note":"fake worker result"})),
    }
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

fn assert_scanned_issue_assignment(msg: WorkerProtocolMessage, issue: ItemNumber) -> Assign {
    match msg {
        WorkerProtocolMessage::Assign(assign) => {
            assert_eq!(assign.repo, "acme/service");
            assert_eq!(assign.role, "engineer");
            assert_eq!(assign.artifact.kind, "issue");
            assert_eq!(assign.artifact.item, json!(issue.get()));
            assert!(assign
                .job_id
                .contains(&format!("/issue-{}/engineer/", issue.get())));
            assign
        }
        other => panic!("expected assign, got {other:?}"),
    }
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

async fn register_engineer(client: &temper_io_engine::http::JsonClient, message_url: &str) {
    assert_eq!(
        post(
            client,
            message_url,
            &register("worker-a", "engineer", "acme/service")
        )
        .await
        .status,
        204
    );
}

async fn wait_for_pull_request_count(
    forge: &MemoryForge,
    repo: &RepositoryId,
    expected: usize,
) -> Vec<PullRequest> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let pulls = forge
            .list_pull_requests(repo, PullRequestQuery::default())
            .await
            .expect("list pull requests succeeds");
        if pulls.len() == expected {
            return pulls;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {expected} pull request(s), saw {}",
            pulls.len()
        );
        temper_io_engine::runtime::sleep_for(Duration::from_millis(10)).await;
    }
}

#[test]
fn posted_webhook_wakes_target_then_worker_is_assigned() {
    temper_io_engine::block_on(async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = create_repo(&forge, "acme", "service", "main").await;
        let issue = create_ready_issue(&forge, &repo).await;
        let workflow = Arc::new(workflow());
        let compiled = Arc::new(workflow.compile());
        let daemon = Daemon::new();
        let config = Arc::new(webhook_config(repo));
        let (message_url, webhook_url) = spawn_with_webhook(
            &daemon,
            forge.clone(),
            workflow,
            compiled,
            Arc::clone(&config),
        )
        .await;
        let client = temper_io_engine::http::JsonClient::new();
        let body = webhook_body(issue);

        assert_eq!(
            post_webhook(&client, &webhook_url, &config.secret, body)
                .await
                .status,
            202
        );

        register_engineer(&client, &message_url).await;
        assert_scanned_issue_assignment(
            post_json(&client, &message_url, &poll("worker-a")).await,
            issue,
        );
    })
}

#[test]
fn posted_webhook_with_invalid_signature_is_unauthorized_and_enqueues_nothing() {
    temper_io_engine::block_on(async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = create_repo(&forge, "acme", "service", "main").await;
        let issue = create_ready_issue(&forge, &repo).await;
        let workflow = Arc::new(workflow());
        let compiled = Arc::new(workflow.compile());
        let daemon = Daemon::new();
        let config = Arc::new(webhook_config(repo));
        let (message_url, webhook_url) = spawn_with_webhook(
            &daemon,
            forge.clone(),
            workflow,
            compiled,
            Arc::clone(&config),
        )
        .await;
        let client = temper_io_engine::http::JsonClient::new();
        let body = webhook_body(issue);

        assert_eq!(
            post_webhook_with_signature(&client, &webhook_url, "sha256=00", body)
                .await
                .status,
            401
        );

        register_engineer(&client, &message_url).await;
        assert_poll_timeout(
            post_json(&client, &message_url, &poll_with_wait("worker-a", 100)).await,
        );
    })
}

#[test]
fn posted_webhook_with_malformed_payload_is_bad_request_and_enqueues_nothing() {
    temper_io_engine::block_on(async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = create_repo(&forge, "acme", "service", "main").await;
        create_ready_issue(&forge, &repo).await;
        let workflow = Arc::new(workflow());
        let compiled = Arc::new(workflow.compile());
        let daemon = Daemon::new();
        let config = Arc::new(webhook_config(repo));
        let (message_url, webhook_url) = spawn_with_webhook(
            &daemon,
            forge.clone(),
            workflow,
            compiled,
            Arc::clone(&config),
        )
        .await;
        let client = temper_io_engine::http::JsonClient::new();
        let body = b"{not valid JSON".to_vec();

        assert_eq!(
            post_webhook(&client, &webhook_url, &config.secret, body)
                .await
                .status,
            400
        );

        register_engineer(&client, &message_url).await;
        assert_poll_timeout(
            post_json(&client, &message_url, &poll_with_wait("worker-a", 100)).await,
        );
    })
}

#[test]
fn posted_webhook_drives_success_apply_to_pull_request() {
    temper_io_engine::block_on(async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = create_repo(&forge, "acme", "service", "stable").await;
        let issue = create_ready_issue(&forge, &repo).await;
        let workflow = Arc::new(workflow());
        let compiled = Arc::new(workflow.compile());
        let daemon = Daemon::with_applier(Arc::new(LeaseApplier::new(
            forge.clone(),
            LeasePolicy::new(chrono::Duration::seconds(300)),
            "daemon-1",
            Arc::new(ForgeApplier::new(forge.clone(), workflow.clone())),
        )));
        let config = Arc::new(webhook_config(repo.clone()));
        let (message_url, webhook_url) = spawn_with_webhook(
            &daemon,
            forge.clone(),
            workflow,
            compiled,
            Arc::clone(&config),
        )
        .await;
        let client = temper_io_engine::http::JsonClient::new();
        let body = webhook_body(issue);

        assert_eq!(
            post_webhook(&client, &webhook_url, &config.secret, body)
                .await
                .status,
            202
        );
        register_engineer(&client, &message_url).await;
        let assignment = assert_scanned_issue_assignment(
            post_json(&client, &message_url, &poll("worker-a")).await,
            issue,
        );

        let summary = "implemented daemon webhook route";
        let branch_name = format!("agent/pr-for-code-{}", issue.get());
        let posted_result = success_result("worker-a", &assignment.job_id, &branch_name, summary);
        assert_release(
            post_json(
                &client,
                &message_url,
                &WorkerProtocolMessage::Result(posted_result),
            )
            .await,
            "worker-a",
            &assignment.job_id,
        );

        let pulls = wait_for_pull_request_count(&forge, &repo, 1).await;
        let pull = &pulls[0];
        assert_eq!(
            pull.title,
            format!("Implement #{}: ready code issue", issue.get())
        );
        assert_eq!(pull.source.repository_id, repo);
        assert_eq!(pull.source.branch, branch_name);
        assert_eq!(pull.target.repository_id, repo);
        assert_eq!(pull.target.branch, "stable");
        assert!(pull.labels.iter().any(|label| label == "implementation"));
        assert!(pull.body.contains(summary));

        let metadata = parse_metadata_block(&pull.body)
            .expect("PR metadata parses")
            .expect("PR metadata exists");
        assert_eq!(
            metadata.kind,
            Some(ArtifactKindId::new("implementation_pr"))
        );
        assert_eq!(metadata.parents, vec![ArtifactRef::same_repo(issue)]);
        let expected_correlation_key = format!("pr-for-code-{}", issue.get());
        assert_eq!(
            metadata.correlation_key.as_deref(),
            Some(expected_correlation_key.as_str())
        );
    })
}
