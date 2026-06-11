// SPDX-License-Identifier: MPL-2.0

use std::{sync::Arc, time::Duration};

use serde_json::json;
use temper_daemon::{Daemon, ForgeApplier, JobContext, LeaseApplier, ResultApplier, RoleFeedMode};
use temper_forge::{
    BranchRef, CreateIssue, CreatePullRequest, CreateRepository, Forge, ItemNumber, PullRequest,
    PullRequestQuery, PullRequestReview, RepositoryId, ReviewDecision, UpdateIssue,
    UpdatePullRequest, UserId,
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
use std::time::Instant;

const REFERENCE_FIXTURE: &str =
    include_str!("../../temper-workflow/fixtures/reference-delivery.json");
const BASIC_FIXTURE: &str = include_str!("../../temper-workflow/fixtures/basic-delivery.json");

fn workflow() -> ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(REFERENCE_FIXTURE).expect("workflow parses");
    spec.validate().expect("workflow validates")
}

fn basic_workflow() -> ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(BASIC_FIXTURE).expect("workflow parses");
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

async fn create_untriaged_intake_issue(forge: &MemoryForge, repo: &RepositoryId) -> ItemNumber {
    forge
        .create_issue(
            repo,
            CreateIssue {
                title: "raw intake".to_string(),
                body: "rough user request".to_string(),
                labels: vec!["untriaged".to_string()],
                assignees: Vec::<UserId>::new(),
            },
        )
        .await
        .expect("intake issue is created")
        .number
}

async fn create_pull_request_needing_review(
    forge: &MemoryForge,
    repo: &RepositoryId,
) -> ItemNumber {
    forge
        .create_pull_request(
            repo,
            CreatePullRequest {
                title: "Implement ready code issue".to_string(),
                body: "Implementation ready for review.".to_string(),
                source: BranchRef {
                    repository_id: repo.clone(),
                    branch: "agent/pr-for-code-1".to_string(),
                },
                target: BranchRef {
                    repository_id: repo.clone(),
                    branch: "stable".to_string(),
                },
                labels: vec!["implementation".to_string(), "needs-reviewer".to_string()],
                assignees: Vec::new(),
            },
        )
        .await
        .expect("pull request is created")
        .number
}

async fn issue_body_and_labels(
    forge: &MemoryForge,
    repo: &RepositoryId,
    number: ItemNumber,
) -> (String, Vec<String>) {
    let issue = forge
        .get_issue_by_number(repo, number)
        .await
        .expect("issue reload succeeds")
        .expect("issue exists");
    (issue.body, issue.labels)
}

async fn spawn(daemon: &Daemon) -> String {
    let server = temper_daemon::serve(&daemon, "127.0.0.1:0".parse().expect("loopback addr"))
        .await
        .expect("bind test server");
    let addr = server.local_addr();
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
        verdict: None,
        body: None,
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
        verdict: None,
        body: None,
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
        verdict: None,
        body: None,
        failure: None,
        summary: Some("done".to_string()),
        details: None,
    }
}

fn verdict_result(worker_id: &str, job_id: &str, verdict: &str, body: Option<&str>) -> JobResult {
    JobResult {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        job_id: job_id.to_string(),
        status: ResultStatus::Success,
        branch: None,
        verdict: Some(verdict.to_string()),
        body: body.map(str::to_string),
        failure: None,
        summary: Some("triaged".to_string()),
        details: None,
    }
}

fn in_flight_job(repo_path: &str, number: ItemNumber) -> InFlightJob {
    job_for_context(
        repo_path,
        number,
        "issue",
        JobContext {
            role: "engineer".to_string(),
            repo: repo_path.to_string(),
            queue: "code_ready".to_string(),
            artifact_kind: "code".to_string(),
            repository: None,
            base_branch: None,
            branch_hint: None,
            correlation_key: None,
            artifact: None,
            action: None,
            checkout_capability: None,
            allowed_verdicts: Vec::new(),
        },
    )
}

fn triage_in_flight_job(repo_path: &str, number: ItemNumber) -> InFlightJob {
    job_for_context(
        repo_path,
        number,
        "issue",
        JobContext {
            role: "architect".to_string(),
            repo: repo_path.to_string(),
            queue: "triage".to_string(),
            artifact_kind: "intake".to_string(),
            repository: None,
            base_branch: None,
            branch_hint: None,
            correlation_key: None,
            artifact: None,
            action: Some("triage_intake".to_string()),
            checkout_capability: Some("read_only".to_string()),
            allowed_verdicts: vec!["ready_code".to_string()],
        },
    )
}

fn review_in_flight_job(repo_path: &str, number: ItemNumber) -> InFlightJob {
    job_for_context(
        repo_path,
        number,
        "pull_request",
        JobContext {
            role: "reviewer".to_string(),
            repo: repo_path.to_string(),
            queue: "pr_needs_review".to_string(),
            artifact_kind: "implementation_pr".to_string(),
            repository: None,
            base_branch: None,
            branch_hint: None,
            correlation_key: None,
            artifact: None,
            action: Some("review_pr".to_string()),
            checkout_capability: Some("pull_request_read_only".to_string()),
            allowed_verdicts: vec![
                "approve".to_string(),
                "changes".to_string(),
                "escalate".to_string(),
            ],
        },
    )
}

fn job_for_context(
    repo_path: &str,
    number: ItemNumber,
    artifact_kind: &str,
    context: JobContext,
) -> InFlightJob {
    InFlightJob {
        job_id: format!(
            "{repo_path}/{artifact_kind}-{}/{}/{}",
            number.get(),
            context.role,
            context.queue
        ),
        role: context.role.clone(),
        repo: repo_path.to_string(),
        artifact: Artifact {
            item: json!(number.get()),
            kind: artifact_kind.to_string(),
        },
        job_payload: serde_json::to_value(context).expect("JobContext serializes"),
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

async fn poll_assignment_for_role(
    client: &temper_io_engine::http::JsonClient,
    url: &str,
    worker_id: &str,
    expected_role: &str,
    expected_artifact_kind: &str,
    number: ItemNumber,
) -> temper_worker_protocol::Assign {
    match post_json(client, url, &poll(worker_id)).await {
        WorkerProtocolMessage::Assign(assign) => {
            assert_eq!(assign.repo, "acme/service");
            assert_eq!(assign.role, expected_role);
            assert_eq!(assign.artifact.kind, expected_artifact_kind);
            assert_eq!(assign.artifact.item, json!(number.get()));
            assign
        }
        other => panic!("expected assign, got {other:?}"),
    }
}

async fn poll_assignment(
    client: &temper_io_engine::http::JsonClient,
    url: &str,
    worker_id: &str,
    issue: ItemNumber,
) -> temper_worker_protocol::Assign {
    poll_assignment_for_role(client, url, worker_id, "engineer", "issue", issue).await
}

async fn poll_review_assignment(
    client: &temper_io_engine::http::JsonClient,
    url: &str,
    worker_id: &str,
    pull_request: ItemNumber,
) -> temper_worker_protocol::Assign {
    poll_assignment_for_role(
        client,
        url,
        worker_id,
        "reviewer",
        "pull_request",
        pull_request,
    )
    .await
}

async fn issue_labels(forge: &MemoryForge, repo: &RepositoryId, number: ItemNumber) -> Vec<String> {
    forge
        .get_issue_by_number(repo, number)
        .await
        .expect("issue reload succeeds")
        .expect("issue exists")
        .labels
}

async fn pull_request_labels(
    forge: &MemoryForge,
    repo: &RepositoryId,
    number: ItemNumber,
) -> Vec<String> {
    forge
        .get_pull_request_by_number(repo, number)
        .await
        .expect("pull request reload succeeds")
        .expect("pull request exists")
        .labels
}

async fn pull_request_reviews(
    forge: &MemoryForge,
    repo: &RepositoryId,
    number: ItemNumber,
) -> Vec<PullRequestReview> {
    let pull_request = forge
        .get_pull_request_by_number(repo, number)
        .await
        .expect("pull request reload succeeds")
        .expect("pull request exists");
    forge
        .list_pull_request_reviews(&pull_request.id)
        .await
        .expect("list pull request reviews succeeds")
}

async fn pull_request_labels_and_reviews(
    forge: &MemoryForge,
    repo: &RepositoryId,
    number: ItemNumber,
) -> (Vec<String>, Vec<PullRequestReview>) {
    (
        pull_request_labels(forge, repo, number).await,
        pull_request_reviews(forge, repo, number).await,
    )
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

async fn drop_issue_label(
    forge: &MemoryForge,
    repo: &RepositoryId,
    number: ItemNumber,
    label: &str,
) {
    let issue = forge
        .get_issue_by_number(repo, number)
        .await
        .expect("issue reload succeeds")
        .expect("issue exists");
    forge
        .update_issue(
            &issue.id,
            UpdateIssue {
                remove_labels: vec![label.to_string()],
                ..UpdateIssue::default()
            },
        )
        .await
        .expect("label is dropped");
}

async fn drop_pull_request_label(
    forge: &MemoryForge,
    repo: &RepositoryId,
    number: ItemNumber,
    label: &str,
) {
    let pull_request = forge
        .get_pull_request_by_number(repo, number)
        .await
        .expect("pull request reload succeeds")
        .expect("pull request exists");
    forge
        .update_pull_request(
            &pull_request.id,
            UpdatePullRequest {
                remove_labels: vec![label.to_string()],
                ..UpdatePullRequest::default()
            },
        )
        .await
        .expect("pull request label is dropped");
}

async fn assert_no_pull_requests(forge: &MemoryForge, repo: &RepositoryId) {
    let pulls = forge
        .list_pull_requests(repo, PullRequestQuery::default())
        .await
        .expect("list pull requests succeeds");
    assert!(pulls.is_empty());
}

fn has_label(labels: &[String], expected: &str) -> bool {
    labels.iter().any(|label| label == expected)
}

async fn wait_for_review_apply(
    forge: &MemoryForge,
    repo: &RepositoryId,
    pull_request: ItemNumber,
    done: impl Fn(&[String], &[PullRequestReview]) -> bool,
) -> (Vec<String>, Vec<PullRequestReview>) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let state = pull_request_labels_and_reviews(forge, repo, pull_request).await;
        if done(&state.0, &state.1) {
            return state;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for review verdict apply, saw labels {:?} reviews {:?}",
            state.0,
            state.1
        );
        temper_io_engine::runtime::sleep_for(Duration::from_millis(10)).await;
    }
}

async fn assert_pull_request_state_stays(
    forge: &MemoryForge,
    repo: &RepositoryId,
    number: ItemNumber,
    expected_labels: Vec<String>,
    expected_reviews: usize,
) {
    let deadline = Instant::now() + Duration::from_millis(250);
    while Instant::now() < deadline {
        let (labels, reviews) = pull_request_labels_and_reviews(forge, repo, number).await;
        assert_eq!(labels, expected_labels);
        assert_eq!(reviews.len(), expected_reviews);
        temper_io_engine::runtime::sleep_for(Duration::from_millis(10)).await;
    }
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
        temper_io_engine::runtime::sleep_for(Duration::from_millis(10)).await;
    }
}

async fn assign_review_job(
    forge: Arc<MemoryForge>,
    repo: &RepositoryId,
    pull_request: ItemNumber,
) -> (
    temper_io_engine::http::JsonClient,
    String,
    temper_worker_protocol::Assign,
) {
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
    let client = temper_io_engine::http::JsonClient::new();
    let role = RoleId::new("reviewer");

    assert_eq!(
        post(
            &client,
            &url,
            &register("worker-a", "reviewer", "acme/service")
        )
        .await
        .status,
        204
    );

    assert_eq!(
        daemon
            .enqueue_scanned_role_work(
                forge.as_ref(),
                repo,
                workflow.as_ref(),
                &compiled,
                ts("2026-05-29T00:00:00Z"),
                &role,
                RoleFeedMode::Normal,
            )
            .await
            .expect("review feed succeeds"),
        1
    );

    let assignment = poll_review_assignment(&client, &url, "worker-a", pull_request).await;
    let context: JobContext = serde_json::from_value(assignment.job_payload.clone())
        .expect("assignment payload is a JobContext");
    assert_eq!(context.role, "reviewer");
    assert_eq!(context.queue, "pr_needs_review");
    assert_eq!(context.artifact_kind, "implementation_pr");
    assert_eq!(context.action.as_deref(), Some("review_pr"));
    assert_eq!(
        context.checkout_capability.as_deref(),
        Some("pull_request_read_only")
    );
    assert_eq!(
        context.allowed_verdicts,
        vec!["approve", "changes", "escalate"]
    );

    (client, url, assignment)
}

#[test]
 fn review_verdict_approve_submits_native_review_and_routes_landing_label() {
    temper_io_engine::block_on(async move {
    let forge = Arc::new(MemoryForge::new());
    let repo = new_repo(&forge, "stable").await;
    let pull_request = create_pull_request_needing_review(&forge, &repo).await;
    let (client, url, assignment) = assign_review_job(forge.clone(), &repo, pull_request).await;

    let result = verdict_result("worker-a", &assignment.job_id, "approve", None);
    assert_release(
        post_json(&client, &url, &WorkerProtocolMessage::Result(result)).await,
        "worker-a",
        &assignment.job_id,
    );

    let (labels, reviews) =
        wait_for_review_apply(&forge, &repo, pull_request, |labels, reviews| {
            !has_label(labels, "needs-reviewer")
                && has_label(labels, "landing")
                && reviews.len() == 1
        })
        .await;

    assert!(has_label(&labels, "implementation"));
    assert!(!has_label(&labels, "needs-reviewer"));
    assert!(has_label(&labels, "landing"));
    assert_eq!(reviews.len(), 1);
    assert_eq!(reviews[0].decision, ReviewDecision::Approved);
    })
}

#[test]
 fn review_verdict_changes_attaches_changes_requested_review_with_body() {
    temper_io_engine::block_on(async move {
    let forge = Arc::new(MemoryForge::new());
    let repo = new_repo(&forge, "stable").await;
    let pull_request = create_pull_request_needing_review(&forge, &repo).await;
    let (client, url, assignment) = assign_review_job(forge.clone(), &repo, pull_request).await;
    let authored = "please add error handling";

    let result = verdict_result("worker-a", &assignment.job_id, "changes", Some(authored));
    assert_release(
        post_json(
            &client,
            &url,
            &WorkerProtocolMessage::Result(result.clone()),
        )
        .await,
        "worker-a",
        &assignment.job_id,
    );

    let (labels, reviews) =
        wait_for_review_apply(&forge, &repo, pull_request, |labels, reviews| {
            !has_label(labels, "needs-reviewer") && reviews.len() == 1
        })
        .await;

    assert!(has_label(&labels, "implementation"));
    assert!(!has_label(&labels, "needs-reviewer"));
    assert_eq!(reviews.len(), 1);
    assert_eq!(reviews[0].decision, ReviewDecision::ChangesRequested);
    let body = reviews[0].body.as_deref().expect("review carries a body");
    assert!(
        body.contains(authored),
        "review body should carry authored text, got `{body}`"
    );

    assert_release(
        post_json(&client, &url, &WorkerProtocolMessage::Result(result)).await,
        "worker-a",
        &assignment.job_id,
    );

    let replay_job = review_in_flight_job("acme/service", pull_request);
    let replay_result = verdict_result("worker-a", &replay_job.job_id, "changes", Some(authored));
    ForgeApplier::new(forge.clone(), Arc::new(workflow()))
        .apply(replay_job, replay_result)
        .await;

    assert_pull_request_state_stays(&forge, &repo, pull_request, labels, 1).await;
    })
}

#[test]
 fn review_verdict_escalate_adds_needs_architect_label() {
    temper_io_engine::block_on(async move {
    let forge = Arc::new(MemoryForge::new());
    let repo = new_repo(&forge, "stable").await;
    let pull_request = create_pull_request_needing_review(&forge, &repo).await;
    let (client, url, assignment) = assign_review_job(forge.clone(), &repo, pull_request).await;

    let result = verdict_result("worker-a", &assignment.job_id, "escalate", None);
    assert_release(
        post_json(&client, &url, &WorkerProtocolMessage::Result(result)).await,
        "worker-a",
        &assignment.job_id,
    );

    let (labels, reviews) =
        wait_for_review_apply(&forge, &repo, pull_request, |labels, reviews| {
            has_label(labels, "needs-architect") && reviews.is_empty()
        })
        .await;

    assert!(has_label(&labels, "implementation"));
    assert!(has_label(&labels, "needs-architect"));
    assert!(reviews.is_empty());
    })
}

#[test]
 fn undeclared_review_verdict_does_not_mutate_pull_request() {
    temper_io_engine::block_on(async move {
    let forge = Arc::new(MemoryForge::new());
    let repo = new_repo(&forge, "stable").await;
    let pull_request = create_pull_request_needing_review(&forge, &repo).await;
    let before = pull_request_labels_and_reviews(&forge, &repo, pull_request).await;
    let (client, url, assignment) = assign_review_job(forge.clone(), &repo, pull_request).await;

    let result = verdict_result("worker-a", &assignment.job_id, "merge_now", None);
    assert_release(
        post_json(&client, &url, &WorkerProtocolMessage::Result(result)).await,
        "worker-a",
        &assignment.job_id,
    );

    assert_pull_request_state_stays(&forge, &repo, pull_request, before.0, before.1.len()).await;
    })
}

#[test]
 fn triage_verdict_success_rewrites_body_and_routes_labels_without_pr() {
    temper_io_engine::block_on(async move {
    let forge = Arc::new(MemoryForge::new());
    let repo = new_repo(&forge, "stable").await;
    let issue = create_untriaged_intake_issue(&forge, &repo).await;
    let workflow = Arc::new(basic_workflow());
    let compiled = workflow.compile();
    let applier = Arc::new(LeaseApplier::new(
        forge.clone(),
        policy(),
        "daemon-1",
        Arc::new(ForgeApplier::new(forge.clone(), workflow.clone())),
    ));
    let daemon = Daemon::with_applier(applier);
    let url = spawn(&daemon).await;
    let client = temper_io_engine::http::JsonClient::new();
    let role = RoleId::new("architect");

    assert_eq!(
        post(
            &client,
            &url,
            &register("worker-a", "architect", "acme/service")
        )
        .await
        .status,
        204
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
    let assignment =
        poll_assignment_for_role(&client, &url, "worker-a", "architect", "issue", issue).await;
    let context: JobContext = serde_json::from_value(assignment.job_payload.clone())
        .expect("assignment payload is a JobContext");
    assert_eq!(context.action.as_deref(), Some("triage_intake"));
    assert_eq!(context.allowed_verdicts, vec!["ready_code"]);
    assert_eq!(context.checkout_capability.as_deref(), Some("read_only"));

    let result = verdict_result(
        "worker-a",
        &assignment.job_id,
        "ready_code",
        Some("rewritten spec"),
    );
    assert_release(
        post_json(&client, &url, &WorkerProtocolMessage::Result(result)).await,
        "worker-a",
        &assignment.job_id,
    );

    let deadline = Instant::now() + Duration::from_secs(2);
    let (body, labels) = loop {
        let state = issue_body_and_labels(&forge, &repo, issue).await;
        if state.0 == "rewritten spec" {
            break state;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for verdict apply, saw body {:?} labels {:?}",
            state.0,
            state.1
        );
        temper_io_engine::runtime::sleep_for(Duration::from_millis(10)).await;
    };

    assert_eq!(body, "rewritten spec");
    assert!(!labels.iter().any(|label| label == "untriaged"));
    assert!(labels.iter().any(|label| label == "code"));
    assert!(labels.iter().any(|label| label == "ready"));
    assert_no_pull_requests(&forge, &repo).await;
    })
}

#[test]
 fn triage_verdict_replay_is_quiet_no_op() {
    temper_io_engine::block_on(async move {
    let forge = Arc::new(MemoryForge::new());
    let repo = new_repo(&forge, "stable").await;
    let issue = create_untriaged_intake_issue(&forge, &repo).await;
    let workflow = Arc::new(basic_workflow());
    let applier = ForgeApplier::new(forge.clone(), workflow);
    let job = triage_in_flight_job("acme/service", issue);
    let result = verdict_result(
        "worker-a",
        &job.job_id,
        "ready_code",
        Some("rewritten spec"),
    );

    applier.apply(job.clone(), result.clone()).await;
    let after_first = issue_body_and_labels(&forge, &repo, issue).await;
    applier.apply(job, result).await;
    let after_second = issue_body_and_labels(&forge, &repo, issue).await;

    assert_eq!(after_first, after_second);
    assert_eq!(after_second.0, "rewritten spec");
    assert!(!after_second.1.iter().any(|label| label == "untriaged"));
    assert!(after_second.1.iter().any(|label| label == "code"));
    assert!(after_second.1.iter().any(|label| label == "ready"));
    assert_no_pull_requests(&forge, &repo).await;
    })
}

#[test]
 fn undeclared_verdict_does_not_mutate_issue() {
    temper_io_engine::block_on(async move {
    let forge = Arc::new(MemoryForge::new());
    let repo = new_repo(&forge, "stable").await;
    let issue = create_untriaged_intake_issue(&forge, &repo).await;
    let workflow = Arc::new(basic_workflow());
    let applier = ForgeApplier::new(forge.clone(), workflow);
    let job = triage_in_flight_job("acme/service", issue);
    let before = issue_body_and_labels(&forge, &repo, issue).await;

    applier
        .apply(
            job.clone(),
            verdict_result("worker-a", &job.job_id, "nonsense", Some("rewritten spec")),
        )
        .await;

    let after = issue_body_and_labels(&forge, &repo, issue).await;
    assert_eq!(after, before);
    assert_no_pull_requests(&forge, &repo).await;
    })
}

#[test]
 fn success_result_creates_implementation_pr_and_replay_is_idempotent() {
    temper_io_engine::block_on(async move {
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
    let client = temper_io_engine::http::JsonClient::new();
    let role = RoleId::new("engineer");

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
    assert_eq!(
        pull.labels,
        vec!["implementation".to_string(), "needs-reviewer".to_string()]
    );
    assert!(pull.body.contains(summary));

    let pull_number = pull.number;
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

    drop_pull_request_label(&forge, &repo, pull_number, "needs-reviewer").await;
    assert_eq!(
        pull_request_labels(&forge, &repo, pull_number).await,
        vec!["implementation".to_string()]
    );

    let replay_job = InFlightJob {
        job_id: assignment.job_id.clone(),
        role: assignment.role.clone(),
        repo: assignment.repo.clone(),
        artifact: assignment.artifact.clone(),
        job_payload: assignment.job_payload.clone(),
    };
    let replay_result = success_result("worker-a", &assignment.job_id, &branch_name, summary);
    ForgeApplier::new(forge.clone(), workflow.clone())
        .apply(replay_job, replay_result)
        .await;
    assert_pull_request_count_stays(&forge, &repo, 1).await;
    assert_eq!(
        pull_request_labels(&forge, &repo, pull_number).await,
        vec!["implementation".to_string()]
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
            .expect("repeat feed succeeds and skips issue with an open implementation PR"),
        0
    );

    assert_pull_request_count_stays(&forge, &repo, 1).await;
    })
}

#[test]
 fn peer_owned_lease_prevents_forge_apply_and_preserves_peer_metadata() {
    temper_io_engine::block_on(async move {
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
    })
}

#[test]
 fn success_without_branch_does_not_create_pull_request_or_mark_issue() {
    temper_io_engine::block_on(async move {
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
    })
}

#[test]
 fn permanent_failure_marks_issue_for_human_attention_and_audit() {
    temper_io_engine::block_on(async move {
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
    assert!(comment.contains(&format!(
        "<!-- temper:comment-key=daemon_failure_audit:{} -->",
        job.job_id
    )));
    })
}

#[test]
 fn failure_marking_applies_for_human_audit_classes() {
    temper_io_engine::block_on(async move {
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
    })
}

#[test]
 fn transient_failure_does_not_create_pull_request_or_mark_issue() {
    temper_io_engine::block_on(async move {
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
    })
}

#[test]
 fn canceled_failure_does_not_create_pull_request_or_mark_issue() {
    temper_io_engine::block_on(async move {
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
    })
}

#[test]
 fn permanent_failure_replay_is_idempotent() {
    temper_io_engine::block_on(async move {
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
    assert!(comments[0].contains(&format!(
        "<!-- temper:comment-key=daemon_failure_audit:{} -->",
        job.job_id
    )));
    })
}

#[test]
 fn permanent_failure_replay_dedupes_by_comment_marker_when_label_is_missing() {
    temper_io_engine::block_on(async move {
    let forge = Arc::new(MemoryForge::new());
    let repo = new_repo(&forge, "stable").await;
    let issue = create_ready_issue(&forge, &repo).await;
    let workflow = Arc::new(workflow());
    let applier = ForgeApplier::new(forge.clone(), workflow);
    let job = in_flight_job("acme/service", issue);
    let result = permanent_failure_result("worker-a", &job.job_id);

    applier.apply(job.clone(), result.clone()).await;
    drop_issue_label(&forge, &repo, issue, "needs-human").await;
    applier.apply(job.clone(), result).await;

    assert_no_pull_requests(&forge, &repo).await;
    assert!(!issue_labels(&forge, &repo, issue)
        .await
        .iter()
        .any(|label| label == "needs-human"));
    let comments = issue_comment_bodies(&forge, &repo, issue).await;
    assert_eq!(comments.len(), 1);
    assert!(comments[0].contains("not implemented"));
    assert!(comments[0].contains(&format!(
        "<!-- temper:comment-key=daemon_failure_audit:{} -->",
        job.job_id
    )));
    })
}
