// SPDX-License-Identifier: MPL-2.0

use std::sync::Arc;

use serde_json::json;
use temper_daemon::{Daemon, JobRepository, RoleFeedMode};
use temper_forge::{
    BranchRef, CreateIssue, CreatePullRequest, CreateRepository, Forge, IssueState, ItemNumber,
    MergeMethod, MergePullRequest, PullRequest, PullRequestUpdateState, RepositoryId, UpdateIssue,
    UpdatePullRequest,
};
use temper_forge_memory::{FaultOp, MemoryForge};
use temper_worker_protocol::{
    Branch, Capability, Capacity, ErrorCode, JobResult, Poll, Register, ReleaseDisposition,
    ResultStatus, WorkerProtocolMessage, WORKER_PROTOCOL_VERSION,
};
use temper_worker_registry::InFlightJob;
use temper_workflow::{
    render_metadata_block, RawWorkflowSpec, RoleId, ValidatedWorkflow, WorkflowMetadata,
};

const FIXTURE: &str = include_str!("../../temper-workflow/fixtures/basic-delivery.json");

struct RecordingApplier {
    tx: temper_io_engine::CqSender<(InFlightJob, JobResult)>,
}

#[async_trait::async_trait]
impl temper_daemon::ResultApplier for RecordingApplier {
    async fn apply(&self, job: InFlightJob, result: JobResult) {
        let _ = self.tx.send((job, result));
    }
}

fn workflow() -> ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(FIXTURE).expect("workflow parses");
    spec.validate().expect("workflow validates")
}

fn ts(value: &str) -> chrono::DateTime<chrono::Utc> {
    value.parse().expect("valid RFC 3339 timestamp")
}

async fn spawn_recording() -> (
    Daemon,
    String,
    temper_io_engine::CqReceiver<(InFlightJob, JobResult)>,
) {
    let (tx, rx) = temper_io_engine::channel();
    let daemon = Daemon::with_applier(Arc::new(RecordingApplier { tx }));
    let server = temper_daemon::serve(&daemon, "127.0.0.1:0".parse().expect("loopback addr"))
        .await
        .expect("bind test server");
    let addr = server.local_addr();
    (daemon, format!("http://{addr}/v1/message"), rx)
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
    let issue = forge
        .create_issue(
            repo,
            CreateIssue {
                title: "ready code issue".to_string(),
                body: "Implement the queued daemon work item.".to_string(),
                labels: labels.iter().map(|label| (*label).to_string()).collect(),
                assignees: Vec::new(),
            },
        )
        .await
        .expect("issue is created");
    issue.number
}

async fn create_issue_record(
    forge: &MemoryForge,
    repo: &RepositoryId,
    labels: &[&str],
) -> temper_forge::Issue {
    forge
        .create_issue(
            repo,
            CreateIssue {
                title: "ready code issue".to_string(),
                body: "Implement the queued daemon work item.".to_string(),
                labels: labels.iter().map(|label| (*label).to_string()).collect(),
                assignees: Vec::new(),
            },
        )
        .await
        .expect("issue is created")
}

async fn create_implementation_pull_request(
    forge: &MemoryForge,
    repo: &RepositoryId,
    correlation_key: &str,
) -> PullRequest {
    forge
        .create_pull_request(
            repo,
            CreatePullRequest {
                title: "Implement queued work".to_string(),
                body: format!(
                    "Implementation PR.\n\n{}",
                    render_metadata_block(&WorkflowMetadata {
                        correlation_key: Some(correlation_key.to_string()),
                        ..WorkflowMetadata::default()
                    })
                ),
                source: BranchRef {
                    repository_id: repo.clone(),
                    branch: format!("agent/{correlation_key}"),
                },
                target: BranchRef {
                    repository_id: repo.clone(),
                    branch: "main".to_string(),
                },
                labels: vec!["implementation".to_string()],
                assignees: Vec::new(),
            },
        )
        .await
        .expect("implementation pull request is created")
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

fn job_result(worker_id: &str, job_id: &str) -> JobResult {
    JobResult {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        job_id: job_id.to_string(),
        status: ResultStatus::Success,
        branch: Some(Branch {
            name: "agent/pr-for-code-feed".to_string(),
            head_sha: "feed123".to_string(),
        }),
        verdict: None,
        body: None,
        children: Vec::new(),
        failure: None,
        summary: Some("done".to_string()),
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

#[test]
 fn scanned_role_work_skips_terminal_labeled_closed_issue() {
    temper_io_engine::block_on(async move {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge).await;
    let issue = create_issue_record(&forge, &repo, &["code", "ready"]).await;
    forge
        .update_issue(
            &issue.id,
            UpdateIssue {
                state: Some(IssueState::Closed),
                ..UpdateIssue::default()
            },
        )
        .await
        .expect("issue is closed");
    let workflow = workflow();
    let compiled = workflow.compile();
    let role = RoleId::new("engineer");
    let (daemon, url, _rx) = spawn_recording().await;
    let client = temper_io_engine::http::JsonClient::new();

    assert_eq!(
        post(
            &client,
            &url,
            &register("worker-closed", "engineer", "acme/service")
        )
        .await
        .status,
        204
    );

    assert_eq!(
        daemon
            .enqueue_scanned_role_work(
                &forge,
                &repo,
                &workflow,
                &compiled,
                ts("2026-05-29T00:00:00Z"),
                &role,
                RoleFeedMode::Normal,
            )
            .await
            .expect("feed succeeds and closed issue is skipped"),
        0
    );
    assert_poll_timeout(post_json(&client, &url, &poll_with_wait("worker-closed", 100)).await);
    })
}

#[test]
 fn scanned_role_work_skips_item_when_enrichment_fails() {
    temper_io_engine::block_on(async move {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge).await;
    let _issue = create_issue(&forge, &repo, &["code", "ready"]).await;
    let workflow = workflow();
    let compiled = workflow.compile();
    let role = RoleId::new("engineer");
    let (daemon, url, _rx) = spawn_recording().await;
    let client = temper_io_engine::http::JsonClient::new();

    assert_eq!(
        post(
            &client,
            &url,
            &register("worker-skip", "engineer", "acme/service")
        )
        .await
        .status,
        204
    );
    forge.fail_next(FaultOp::GetIssueByNumber, "issue snapshot lookup failed");

    assert_eq!(
        daemon
            .enqueue_scanned_role_work(
                &forge,
                &repo,
                &workflow,
                &compiled,
                ts("2026-05-29T00:00:00Z"),
                &role,
                RoleFeedMode::Normal,
            )
            .await
            .expect("scan succeeds and enrichment failure is skipped"),
        0
    );
    assert_poll_timeout(post_json(&client, &url, &poll_with_wait("worker-skip", 100)).await);
    })
}

#[test]
 fn scanned_writable_issue_skips_while_open_pr_has_correlation_key() {
    temper_io_engine::block_on(async move {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge).await;
    let issue = create_issue(&forge, &repo, &["code", "ready"]).await;
    let workflow = workflow();
    let compiled = workflow.compile();
    let role = RoleId::new("engineer");
    let correlation_key = format!("pr-for-code-{}", issue.get());
    let _pull_request = create_implementation_pull_request(&forge, &repo, &correlation_key).await;
    let (daemon, url, _rx) = spawn_recording().await;
    let client = temper_io_engine::http::JsonClient::new();

    assert_eq!(
        post(
            &client,
            &url,
            &register("worker-existing-pr", "engineer", "acme/service")
        )
        .await
        .status,
        204
    );

    assert_eq!(
        daemon
            .enqueue_scanned_role_work(
                &forge,
                &repo,
                &workflow,
                &compiled,
                ts("2026-05-29T00:00:00Z"),
                &role,
                RoleFeedMode::Normal,
            )
            .await
            .expect("feed succeeds and open correlated PR is skipped"),
        0
    );
    assert_poll_timeout(post_json(&client, &url, &poll_with_wait("worker-existing-pr", 100)).await);
    })
}

#[test]
 fn scanned_writable_issue_skips_while_merged_pr_has_correlation_key() {
    temper_io_engine::block_on(async move {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge).await;
    let issue = create_issue(&forge, &repo, &["code", "ready"]).await;
    let workflow = workflow();
    let compiled = workflow.compile();
    let role = RoleId::new("engineer");
    let correlation_key = format!("pr-for-code-{}", issue.get());
    let pull_request = create_implementation_pull_request(&forge, &repo, &correlation_key).await;
    forge
        .merge_pull_request(
            &pull_request.id,
            MergePullRequest {
                method: MergeMethod::Squash,
                commit_title: None,
                commit_body: None,
            },
        )
        .await
        .expect("pull request is merged");
    let (daemon, url, _rx) = spawn_recording().await;
    let client = temper_io_engine::http::JsonClient::new();

    assert_eq!(
        post(
            &client,
            &url,
            &register("worker-merged-pr", "engineer", "acme/service")
        )
        .await
        .status,
        204
    );

    assert_eq!(
        daemon
            .enqueue_scanned_role_work(
                &forge,
                &repo,
                &workflow,
                &compiled,
                ts("2026-05-29T00:00:00Z"),
                &role,
                RoleFeedMode::Normal,
            )
            .await
            .expect("feed succeeds and merged correlated PR is skipped"),
        0
    );
    assert_poll_timeout(post_json(&client, &url, &poll_with_wait("worker-merged-pr", 100)).await);
    })
}

#[test]
 fn scanned_writable_issue_enqueues_after_correlated_pr_closes_unmerged() {
    temper_io_engine::block_on(async move {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge).await;
    let issue = create_issue(&forge, &repo, &["code", "ready"]).await;
    let workflow = workflow();
    let compiled = workflow.compile();
    let role = RoleId::new("engineer");
    let correlation_key = format!("pr-for-code-{}", issue.get());
    let pull_request = create_implementation_pull_request(&forge, &repo, &correlation_key).await;
    forge
        .update_pull_request(
            &pull_request.id,
            UpdatePullRequest {
                state: Some(PullRequestUpdateState::Closed),
                ..UpdatePullRequest::default()
            },
        )
        .await
        .expect("pull request is closed unmerged");
    let (daemon, url, _rx) = spawn_recording().await;
    let client = temper_io_engine::http::JsonClient::new();

    assert_eq!(
        post(
            &client,
            &url,
            &register("worker-closed-unmerged-pr", "engineer", "acme/service")
        )
        .await
        .status,
        204
    );

    assert_eq!(
        daemon
            .enqueue_scanned_role_work(
                &forge,
                &repo,
                &workflow,
                &compiled,
                ts("2026-05-29T00:00:00Z"),
                &role,
                RoleFeedMode::Normal,
            )
            .await
            .expect("feed succeeds after correlated PR closes unmerged"),
        1
    );

    match post_json(&client, &url, &poll("worker-closed-unmerged-pr")).await {
        WorkerProtocolMessage::Assign(assign) => {
            assert_eq!(assign.role, "engineer");
            assert_eq!(assign.artifact.kind, "issue");
            assert_eq!(assign.artifact.item, json!(issue.get()));
        }
        other => panic!("expected assign after closing correlated PR unmerged, got {other:?}"),
    }
    })
}

#[test]
 fn scanned_read_only_triage_item_enqueues_even_when_open_pr_exists() {
    temper_io_engine::block_on(async move {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge).await;
    let issue = create_issue(&forge, &repo, &["untriaged"]).await;
    let _pull_request = create_implementation_pull_request(&forge, &repo, "pr-for-code-999").await;
    let workflow = workflow();
    let compiled = workflow.compile();
    let role = RoleId::new("architect");
    let (daemon, url, _rx) = spawn_recording().await;
    let client = temper_io_engine::http::JsonClient::new();

    assert_eq!(
        post(
            &client,
            &url,
            &register("worker-triage-open-pr", "architect", "acme/service")
        )
        .await
        .status,
        204
    );

    assert_eq!(
        daemon
            .enqueue_scanned_role_work(
                &forge,
                &repo,
                &workflow,
                &compiled,
                ts("2026-05-29T00:00:00Z"),
                &role,
                RoleFeedMode::Normal,
            )
            .await
            .expect("feed succeeds"),
        1
    );

    match post_json(&client, &url, &poll("worker-triage-open-pr")).await {
        WorkerProtocolMessage::Assign(assign) => {
            assert_eq!(assign.role, "architect");
            assert_eq!(assign.artifact.kind, "issue");
            assert_eq!(assign.artifact.item, json!(issue.get()));
            let context: temper_daemon::JobContext =
                serde_json::from_value(assign.job_payload).expect("triage payload parses");
            assert_eq!(context.checkout_capability.as_deref(), Some("read_only"));
        }
        other => panic!("expected triage assign, got {other:?}"),
    }
    })
}

#[test]
 fn scanned_architect_triage_item_carries_verdict_job_enrichment() {
    temper_io_engine::block_on(async move {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge).await;
    let issue = create_issue(&forge, &repo, &["untriaged"]).await;
    let workflow = workflow();
    let compiled = workflow.compile();
    let role = RoleId::new("architect");
    let (daemon, url, _rx) = spawn_recording().await;
    let client = temper_io_engine::http::JsonClient::new();

    assert_eq!(
        daemon
            .enqueue_scanned_role_work(
                &forge,
                &repo,
                &workflow,
                &compiled,
                ts("2026-05-29T00:00:00Z"),
                &role,
                RoleFeedMode::Normal,
            )
            .await
            .expect("feed succeeds"),
        1
    );

    assert_eq!(
        post(
            &client,
            &url,
            &register("worker-architect", "architect", "acme/service")
        )
        .await
        .status,
        204
    );

    let assignment = match post_json(&client, &url, &poll("worker-architect")).await {
        WorkerProtocolMessage::Assign(assign) => {
            assert_eq!(assign.repo, "acme/service");
            assert_eq!(assign.role, "architect");
            assert_eq!(assign.artifact.kind, "issue");
            assert_eq!(assign.artifact.item, json!(issue.get()));
            assert!(assign
                .job_id
                .contains(&format!("/issue-{}/architect/triage", issue.get())));
            assign
        }
        other => panic!("expected assign, got {other:?}"),
    };

    let context: temper_daemon::JobContext = serde_json::from_value(assignment.job_payload)
        .expect("assign job payload parses as daemon-reexported JobContext");
    assert_eq!(context.role, "architect");
    assert_eq!(context.repo, "acme/service");
    assert_eq!(context.queue, "triage");
    assert_eq!(context.artifact_kind, "intake");
    assert_eq!(context.action.as_deref(), Some("triage_intake"));
    assert_eq!(context.checkout_capability.as_deref(), Some("read_only"));
    assert_eq!(context.allowed_verdicts, vec!["ready_code".to_string()]);
    assert_eq!(context.base_branch.as_deref(), Some("main"));
    let artifact = context.artifact.expect("issue snapshot is present");
    assert_eq!(artifact.number, issue.get());
    assert_eq!(artifact.labels, vec!["untriaged".to_string()]);
    })
}

#[test]
 fn scanned_role_work_dispatches_to_worker_and_applies_once() {
    temper_io_engine::block_on(async move {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge).await;
    let issue = create_issue(&forge, &repo, &["code", "ready"]).await;
    let workflow = workflow();
    let compiled = workflow.compile();
    let role = RoleId::new("engineer");
    let (daemon, url, mut rx) = spawn_recording().await;
    let client = temper_io_engine::http::JsonClient::new();

    assert_eq!(
        daemon
            .enqueue_scanned_role_work(
                &forge,
                &repo,
                &workflow,
                &compiled,
                ts("2026-05-29T00:00:00Z"),
                &role,
                RoleFeedMode::Normal,
            )
            .await
            .expect("feed succeeds"),
        1
    );
    assert_eq!(
        daemon
            .enqueue_scanned_role_work(
                &forge,
                &repo,
                &workflow,
                &compiled,
                ts("2026-05-29T00:00:00Z"),
                &role,
                RoleFeedMode::Normal,
            )
            .await
            .expect("repeat feed succeeds"),
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

    let assignment = match post_json(&client, &url, &poll("worker-a")).await {
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
    };
    let context: temper_daemon::JobContext = serde_json::from_value(assignment.job_payload.clone())
        .expect("assign job payload parses as daemon-reexported JobContext");
    assert_eq!(context.role, "engineer");
    assert_eq!(context.repo, "acme/service");
    assert_eq!(context.queue, "code_ready");
    assert_eq!(context.artifact_kind, "code");
    assert_eq!(
        context.repository,
        Some(JobRepository {
            owner: "acme".to_string(),
            name: "service".to_string(),
            default_branch: "main".to_string(),
        })
    );
    assert_eq!(context.base_branch.as_deref(), Some("main"));
    assert_eq!(context.action.as_deref(), Some("open_pr"));
    assert_eq!(context.checkout_capability.as_deref(), Some("writable"));
    assert!(context.allowed_verdicts.is_empty());
    let expected_branch_hint = format!("agent/pr-for-code-{}", issue.get());
    let expected_correlation_key = format!("pr-for-code-{}", issue.get());
    assert_eq!(
        context.branch_hint.as_deref(),
        Some(expected_branch_hint.as_str())
    );
    assert_eq!(
        context.correlation_key.as_deref(),
        Some(expected_correlation_key.as_str())
    );
    let artifact = context.artifact.expect("issue snapshot is present");
    assert_eq!(artifact.number, issue.get());
    assert_eq!(artifact.title, "ready code issue");
    assert_eq!(artifact.body, "Implement the queued daemon work item.");
    assert_eq!(
        artifact.labels,
        vec!["code".to_string(), "ready".to_string()]
    );
    assert_eq!(artifact.state, "Open");

    let posted_result = job_result("worker-a", &assignment.job_id);
    match post_json(
        &client,
        &url,
        &WorkerProtocolMessage::Result(posted_result.clone()),
    )
    .await
    {
        WorkerProtocolMessage::Release(release) => {
            assert_eq!(release.worker_id, "worker-a");
            assert_eq!(release.job_id, assignment.job_id);
            assert_eq!(release.disposition, ReleaseDisposition::Accepted);
        }
        other => panic!("expected release, got {other:?}"),
    }

    let (job, recorded_result) = rx.recv().await.expect("applier records accepted result");
    assert_eq!(job.job_id, assignment.job_id);
    assert_eq!(job.repo, "acme/service");
    assert_eq!(job.role, "engineer");
    assert_eq!(job.artifact.kind, "issue");
    assert_eq!(job.artifact.item, json!(issue.get()));
    assert_eq!(job.job_payload, assignment.job_payload);
    assert_eq!(recorded_result, posted_result);
    assert_eq!(recorded_result.status, ResultStatus::Success);
    assert!(rx.try_recv().is_none());

    assert_poll_timeout(post_json(&client, &url, &poll_with_wait("worker-a", 100)).await);
    assert!(rx.try_recv().is_none());
    })
}
