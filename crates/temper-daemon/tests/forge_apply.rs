// SPDX-License-Identifier: MPL-2.0

use std::{future::IntoFuture, sync::Arc, time::Duration};

use axum::http::StatusCode;
use serde_json::json;
use temper_daemon::{Daemon, ForgeApplier, JobContext, LeaseApplier, ResultApplier, RoleFeedMode};
use temper_forge::{
    CreateIssue, CreateRepository, Forge, ItemNumber, PullRequest, PullRequestQuery, RepositoryId,
    UserId,
};
use temper_forge_memory::MemoryForge;
use temper_worker_protocol::{
    Artifact, Branch, Capability, Capacity, Failure, FailureClass, JobResult, Poll, Register,
    ReleaseDisposition, ResultStatus, WorkerProtocolMessage, WORKER_PROTOCOL_VERSION,
};
use temper_worker_registry::InFlightJob;
use temper_workflow::{
    parse_metadata_block, ArtifactKindId, ArtifactRef, ArtifactSource, LeaseManager, LeasePolicy,
    RawWorkflowSpec, RoleId, ValidatedWorkflow,
};
use tokio::time::{sleep, Instant};

const FIXTURE: &str = include_str!("../../temper-workflow/fixtures/reference-delivery.json");

fn workflow() -> ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(FIXTURE).expect("workflow parses");
    spec.validate().expect("workflow validates")
}

fn ts(value: &str) -> chrono::DateTime<chrono::Utc> {
    value.parse().expect("valid RFC 3339 timestamp")
}

fn policy() -> LeasePolicy {
    LeasePolicy::new(chrono::Duration::seconds(300))
}

async fn new_repo(forge: &MemoryForge, default_branch: &str) -> RepositoryId {
    forge
        .create_repository(CreateRepository {
            owner: "acme".to_string(),
            name: "service".to_string(),
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
                body: "Implement the daemon worker apply path.".to_string(),
                labels: vec!["code".to_string(), "ready".to_string()],
                assignees: Vec::<UserId>::new(),
            },
        )
        .await
        .expect("issue is created")
        .number
}

async fn spawn(daemon: &Daemon) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let addr = listener.local_addr().expect("read local addr");
    tokio::spawn(axum::serve(listener, daemon.router()).into_future());
    format!("http://{addr}/v1/message")
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

fn poll(worker_id: &str) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Poll(Poll {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        free_capacity: 1,
        max_wait_ms: Some(30_000),
    })
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
        failure: None,
        summary: Some(summary.to_string()),
        details: Some(json!({"note":"fake worker result"})),
    }
}

fn failure_result(
    worker_id: &str,
    job_id: &str,
    failure_class: Option<FailureClass>,
    message: &str,
) -> JobResult {
    JobResult {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        job_id: job_id.to_string(),
        status: ResultStatus::Failure,
        branch: Some(Branch {
            name: "agent/pr-for-code-1".to_string(),
            head_sha: "def456".to_string(),
        }),
        failure: failure_class.map(|class| Failure {
            class,
            message: message.to_string(),
        }),
        summary: Some("failed".to_string()),
        details: None,
    }
}

fn permanent_failure_result(worker_id: &str, job_id: &str) -> JobResult {
    failure_result(
        worker_id,
        job_id,
        Some(FailureClass::Permanent),
        "not implemented",
    )
}

fn success_without_branch(worker_id: &str, job_id: &str) -> JobResult {
    JobResult {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        job_id: job_id.to_string(),
        status: ResultStatus::Success,
        branch: None,
        failure: None,
        summary: Some("done".to_string()),
        details: None,
    }
}

fn in_flight_job(repo_path: &str, number: ItemNumber) -> InFlightJob {
    InFlightJob {
        job_id: format!("{repo_path}/issue-{}/engineer/code_ready", number.get()),
        role: "engineer".to_string(),
        repo: repo_path.to_string(),
        artifact: Artifact {
            item: json!(number.get()),
            kind: "issue".to_string(),
        },
        job_payload: serde_json::to_value(JobContext {
            role: "engineer".to_string(),
            repo: repo_path.to_string(),
            queue: "code_ready".to_string(),
            artifact_kind: "code".to_string(),
        })
        .expect("JobContext serializes"),
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

async fn poll_assignment(
    client: &reqwest::Client,
    url: &str,
    worker_id: &str,
    issue: ItemNumber,
) -> temper_worker_protocol::Assign {
    match post_json(client, url, &poll(worker_id)).await {
        WorkerProtocolMessage::Assign(assign) => {
            assert_eq!(assign.repo, "acme/service");
            assert_eq!(assign.role, "engineer");
            assert_eq!(assign.artifact.kind, "issue");
            assert_eq!(assign.artifact.item, json!(issue.get()));
            assign
        }
        other => panic!("expected assign, got {other:?}"),
    }
}

async fn issue_labels(forge: &MemoryForge, repo: &RepositoryId, number: ItemNumber) -> Vec<String> {
    forge
        .get_issue_by_number(repo, number)
        .await
        .expect("issue reload succeeds")
        .expect("issue exists")
        .labels
}

async fn issue_comment_bodies(
    forge: &MemoryForge,
    repo: &RepositoryId,
    number: ItemNumber,
) -> Vec<String> {
    let issue = forge
        .get_issue_by_number(repo, number)
        .await
        .expect("issue reload succeeds")
        .expect("issue exists");
    forge
        .list_issue_comments(&issue.id)
        .await
        .expect("list issue comments succeeds")
        .into_iter()
        .map(|comment| comment.body)
        .collect()
}

async fn assert_no_attention_mark(forge: &MemoryForge, repo: &RepositoryId, issue: ItemNumber) {
    assert!(!issue_labels(forge, repo, issue)
        .await
        .iter()
        .any(|label| label == "needs-human"));
    assert!(issue_comment_bodies(forge, repo, issue).await.is_empty());
}

async fn assert_no_pull_requests(forge: &MemoryForge, repo: &RepositoryId) {
    let pulls = forge
        .list_pull_requests(repo, PullRequestQuery::default())
        .await
        .expect("list pull requests succeeds");
    assert!(pulls.is_empty());
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
        sleep(Duration::from_millis(10)).await;
    }
}

async fn assert_pull_request_count_stays(
    forge: &MemoryForge,
    repo: &RepositoryId,
    expected: usize,
) {
    let deadline = Instant::now() + Duration::from_millis(250);
    while Instant::now() < deadline {
        let pulls = forge
            .list_pull_requests(repo, PullRequestQuery::default())
            .await
            .expect("list pull requests succeeds");
        assert_eq!(pulls.len(), expected);
        sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn success_result_creates_implementation_pr_and_replay_is_idempotent() {
    let forge = Arc::new(MemoryForge::new());
    let repo = new_repo(&forge, "stable").await;
    let issue = create_ready_issue(&forge, &repo).await;
    let workflow = Arc::new(workflow());
    let compiled = workflow.compile();
    let applier = Arc::new(LeaseApplier::new(
        forge.clone(),
        policy(),
        "daemon-1",
        Arc::new(ForgeApplier::new(forge.clone(), workflow.clone())),
    ));
    let daemon = Daemon::with_applier(applier);
    let url = spawn(&daemon).await;
    let client = reqwest::Client::new();
    let role = RoleId::new("engineer");

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

    assert_eq!(
        daemon
            .enqueue_scanned_role_work(
                forge.as_ref(),
                &repo,
                workflow.as_ref(),
                &compiled,
                ts("2026-05-29T00:00:00Z"),
                &role,
                RoleFeedMode::Normal,
            )
            .await
            .expect("feed succeeds"),
        1
    );
    let assignment = poll_assignment(&client, &url, "worker-a", issue).await;
    let summary = "implemented daemon worker success apply";
    let branch_name = format!("agent/pr-for-code-{}", issue.get());
    let posted_result = success_result("worker-a", &assignment.job_id, &branch_name, summary);
    assert_release(
        post_json(&client, &url, &WorkerProtocolMessage::Result(posted_result)).await,
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
    assert!(pull.assignees.is_empty());
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

    assert_eq!(
        daemon
            .enqueue_scanned_role_work(
                forge.as_ref(),
                &repo,
                workflow.as_ref(),
                &compiled,
                ts("2026-05-29T00:00:00Z"),
                &role,
                RoleFeedMode::Normal,
            )
            .await
            .expect("repeat feed succeeds"),
        1
    );
    let replay_assignment = poll_assignment(&client, &url, "worker-a", issue).await;
    let replay_result =
        success_result("worker-a", &replay_assignment.job_id, &branch_name, summary);
    assert_release(
        post_json(&client, &url, &WorkerProtocolMessage::Result(replay_result)).await,
        "worker-a",
        &replay_assignment.job_id,
    );

    assert_pull_request_count_stays(&forge, &repo, 1).await;
}

#[tokio::test]
async fn peer_owned_lease_prevents_forge_apply_and_preserves_peer_metadata() {
    let forge = Arc::new(MemoryForge::new());
    let repo = new_repo(&forge, "stable").await;
    let issue = create_ready_issue(&forge, &repo).await;
    let workflow = Arc::new(workflow());
    let target = ArtifactSource::Issue { number: issue };
    let manager = LeaseManager::new(forge.as_ref(), policy());
    let peer_lease = manager
        .acquire(
            &repo,
            target,
            RoleId::new("engineer"),
            "peer-daemon",
            chrono::Utc::now(),
        )
        .await
        .expect("peer lease is acquired");
    let applier = LeaseApplier::new(
        forge.clone(),
        policy(),
        "daemon-1",
        Arc::new(ForgeApplier::new(forge.clone(), workflow)),
    );
    let job = in_flight_job("acme/service", issue);
    let result = success_result(
        "worker-a",
        &job.job_id,
        &format!("agent/pr-for-code-{}", issue.get()),
        "done",
    );

    applier.apply(job, result).await;

    let pulls = forge
        .list_pull_requests(&repo, PullRequestQuery::default())
        .await
        .expect("list pull requests succeeds");
    assert!(pulls.is_empty());
    let issue = forge
        .get_issue_by_number(&repo, issue)
        .await
        .expect("issue reload succeeds")
        .expect("issue exists after apply");
    let lease = parse_metadata_block(&issue.body)
        .expect("issue metadata parses")
        .expect("issue has metadata")
        .lease
        .expect("peer lease is still present");
    assert_eq!(lease, peer_lease);
}

#[tokio::test]
async fn success_without_branch_does_not_create_pull_request_or_mark_issue() {
    let forge = Arc::new(MemoryForge::new());
    let repo = new_repo(&forge, "stable").await;
    let issue = create_ready_issue(&forge, &repo).await;
    let workflow = Arc::new(workflow());
    let applier = ForgeApplier::new(forge.clone(), workflow);
    let job = in_flight_job("acme/service", issue);

    applier
        .apply(job.clone(), success_without_branch("worker-a", &job.job_id))
        .await;

    assert_no_pull_requests(&forge, &repo).await;
    assert_no_attention_mark(&forge, &repo, issue).await;
}

#[tokio::test]
async fn permanent_failure_marks_issue_for_human_attention_and_audit() {
    let forge = Arc::new(MemoryForge::new());
    let repo = new_repo(&forge, "stable").await;
    let issue = create_ready_issue(&forge, &repo).await;
    let workflow = Arc::new(workflow());
    let applier = ForgeApplier::new(forge.clone(), workflow);
    let job = in_flight_job("acme/service", issue);

    applier
        .apply(
            job.clone(),
            permanent_failure_result("worker-a", &job.job_id),
        )
        .await;

    assert_no_pull_requests(&forge, &repo).await;
    let labels = issue_labels(&forge, &repo, issue).await;
    assert!(labels.iter().any(|label| label == "needs-human"));
    let comments = issue_comment_bodies(&forge, &repo, issue).await;
    assert_eq!(comments.len(), 1);
    let comment = &comments[0];
    assert!(comment.contains("not implemented"));
    assert!(comment.contains("failure class: permanent"));
    assert!(comment.contains(&format!("job_id: `{}`", job.job_id)));
    assert!(comment.contains("worker: `worker-a`"));
}

#[tokio::test]
async fn failure_marking_applies_for_human_audit_classes() {
    for (failure_class, expected_class, message) in [
        (
            Some(FailureClass::Permanent),
            "permanent",
            "permanent worker failure",
        ),
        (
            Some(FailureClass::Protocol),
            "protocol",
            "protocol worker failure",
        ),
        (None, "unknown", "missing failure details"),
    ] {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_ready_issue(&forge, &repo).await;
        let workflow = Arc::new(workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow);
        let job = in_flight_job("acme/service", issue);

        applier
            .apply(
                job.clone(),
                failure_result("worker-a", &job.job_id, failure_class, message),
            )
            .await;

        assert_no_pull_requests(&forge, &repo).await;
        let labels = issue_labels(&forge, &repo, issue).await;
        assert!(labels.iter().any(|label| label == "needs-human"));
        let comments = issue_comment_bodies(&forge, &repo, issue).await;
        assert_eq!(comments.len(), 1);
        let comment = &comments[0];
        assert!(comment.contains(&format!("failure class: {expected_class}")));
        assert!(comment.contains(&format!("job_id: `{}`", job.job_id)));
        assert!(comment.contains("worker: `worker-a`"));
        if failure_class.is_some() {
            assert!(comment.contains(message));
        } else {
            assert!(!comment.contains(message));
        }
    }
}

#[tokio::test]
async fn transient_failure_does_not_create_pull_request_or_mark_issue() {
    let forge = Arc::new(MemoryForge::new());
    let repo = new_repo(&forge, "stable").await;
    let issue = create_ready_issue(&forge, &repo).await;
    let workflow = Arc::new(workflow());
    let applier = ForgeApplier::new(forge.clone(), workflow);
    let job = in_flight_job("acme/service", issue);

    applier
        .apply(
            job.clone(),
            failure_result(
                "worker-a",
                &job.job_id,
                Some(FailureClass::Transient),
                "try again later",
            ),
        )
        .await;

    assert_no_pull_requests(&forge, &repo).await;
    assert_no_attention_mark(&forge, &repo, issue).await;
}

#[tokio::test]
async fn canceled_failure_does_not_create_pull_request_or_mark_issue() {
    let forge = Arc::new(MemoryForge::new());
    let repo = new_repo(&forge, "stable").await;
    let issue = create_ready_issue(&forge, &repo).await;
    let workflow = Arc::new(workflow());
    let applier = ForgeApplier::new(forge.clone(), workflow);
    let job = in_flight_job("acme/service", issue);

    applier
        .apply(
            job.clone(),
            failure_result(
                "worker-a",
                &job.job_id,
                Some(FailureClass::Canceled),
                "worker stopped",
            ),
        )
        .await;

    assert_no_pull_requests(&forge, &repo).await;
    assert_no_attention_mark(&forge, &repo, issue).await;
}

#[tokio::test]
async fn permanent_failure_replay_is_idempotent() {
    let forge = Arc::new(MemoryForge::new());
    let repo = new_repo(&forge, "stable").await;
    let issue = create_ready_issue(&forge, &repo).await;
    let workflow = Arc::new(workflow());
    let applier = ForgeApplier::new(forge.clone(), workflow);
    let job = in_flight_job("acme/service", issue);
    let result = permanent_failure_result("worker-a", &job.job_id);

    applier.apply(job.clone(), result.clone()).await;
    applier.apply(job.clone(), result).await;

    assert_no_pull_requests(&forge, &repo).await;
    let labels = issue_labels(&forge, &repo, issue).await;
    assert_eq!(
        labels
            .iter()
            .filter(|label| label.as_str() == "needs-human")
            .count(),
        1
    );
    let comments = issue_comment_bodies(&forge, &repo, issue).await;
    assert_eq!(comments.len(), 1);
    assert!(comments[0].contains("not implemented"));
}
