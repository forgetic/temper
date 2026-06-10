// SPDX-License-Identifier: MPL-2.0

use std::{future::IntoFuture, sync::Arc};

use axum::http::StatusCode;
use serde_json::json;
use temper_daemon::{Daemon, JobRepository, RoleFeedMode};
use temper_forge::{CreateIssue, CreateRepository, Forge, ItemNumber, RepositoryId};
use temper_forge_memory::{FaultOp, MemoryForge};
use temper_worker_protocol::{
    Branch, Capability, Capacity, ErrorCode, JobResult, Poll, Register, ReleaseDisposition,
    ResultStatus, WorkerProtocolMessage, WORKER_PROTOCOL_VERSION,
};
use temper_worker_registry::InFlightJob;
use temper_workflow::{RawWorkflowSpec, RoleId, ValidatedWorkflow};
use tokio::sync::mpsc;

const FIXTURE: &str = include_str!("../../temper-workflow/fixtures/basic-delivery.json");

struct RecordingApplier {
    tx: mpsc::UnboundedSender<(InFlightJob, JobResult)>,
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
    mpsc::UnboundedReceiver<(InFlightJob, JobResult)>,
) {
    let (tx, rx) = mpsc::unbounded_channel();
    let daemon = Daemon::with_applier(Arc::new(RecordingApplier { tx }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let addr = listener.local_addr().expect("read local addr");
    tokio::spawn(axum::serve(listener, daemon.router()).into_future());
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
        failure: None,
        summary: Some("done".to_string()),
        details: Some(json!({"note":"fake worker result"})),
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

fn assert_poll_timeout(msg: WorkerProtocolMessage) {
    match msg {
        WorkerProtocolMessage::Error(error) => assert_eq!(error.code, ErrorCode::PollTimeout),
        other => panic!("expected poll timeout, got {other:?}"),
    }
}

#[tokio::test]
async fn scanned_role_work_skips_item_when_enrichment_fails() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge).await;
    let _issue = create_issue(&forge, &repo, &["code", "ready"]).await;
    let workflow = workflow();
    let compiled = workflow.compile();
    let role = RoleId::new("engineer");
    let (daemon, url, _rx) = spawn_recording().await;
    let client = reqwest::Client::new();

    assert_eq!(
        post(
            &client,
            &url,
            &register("worker-skip", "engineer", "acme/service")
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
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
}

#[tokio::test]
async fn scanned_architect_triage_item_carries_verdict_job_enrichment() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge).await;
    let issue = create_issue(&forge, &repo, &["untriaged"]).await;
    let workflow = workflow();
    let compiled = workflow.compile();
    let role = RoleId::new("architect");
    let (daemon, url, _rx) = spawn_recording().await;
    let client = reqwest::Client::new();

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
        .status(),
        StatusCode::NO_CONTENT
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
}

#[tokio::test]
async fn scanned_role_work_dispatches_to_worker_and_applies_once() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge).await;
    let issue = create_issue(&forge, &repo, &["code", "ready"]).await;
    let workflow = workflow();
    let compiled = workflow.compile();
    let role = RoleId::new("engineer");
    let (daemon, url, mut rx) = spawn_recording().await;
    let client = reqwest::Client::new();

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
        .status(),
        StatusCode::NO_CONTENT
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
    assert!(matches!(
        rx.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));

    assert_poll_timeout(post_json(&client, &url, &poll_with_wait("worker-a", 100)).await);
    assert!(matches!(
        rx.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
}
