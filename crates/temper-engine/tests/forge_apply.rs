// SPDX-License-Identifier: MPL-2.0

use std::{sync::Arc, time::Duration};

use serde_json::json;
use std::time::Instant;
use temper_engine::{Daemon, ForgeApplier, JobContext, LeaseApplier, ResultApplier, RoleFeedMode};
use temper_forge::{
    BranchRef, CreateIssue, CreatePullRequest, CreateRepository, Forge, Issue, IssueQuery,
    ItemNumber, PullRequest, PullRequestQuery, PullRequestReview, RepositoryId, ReviewDecision,
    UpdateIssue, UpdatePullRequest, UserId,
};
use temper_forge_memory::MemoryForge;
use temper_worker_protocol::{
    Artifact, Branch, Capability, Capacity, Failure, FailureClass, JobChild, JobResult, Poll,
    Register, ReleaseDisposition, RepoAccess, RepoOutcome, ResultStatus, WORKER_PROTOCOL_VERSION,
    WorkerProtocolMessage, WorkspaceManifest, WorkspaceRepo,
};
use temper_worker_registry::InFlightJob;
use temper_workflow::{
    ArtifactKindId, ArtifactRef, ArtifactSource, LeaseManager, LeasePolicy, RawWorkflowSpec,
    RoleId, ValidatedWorkflow, global_child_correlation_key, parse_metadata_block,
};

const REFERENCE_FIXTURE: &str =
    include_str!("../../temper-workflow/fixtures/reference-delivery.json");

fn workflow() -> ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(REFERENCE_FIXTURE).expect("workflow parses");
    spec.validate().expect("workflow validates")
}

fn ts(value: &str) -> chrono::DateTime<chrono::Utc> {
    value.parse().expect("valid RFC 3339 timestamp")
}

fn policy() -> LeasePolicy {
    LeasePolicy::new(chrono::Duration::seconds(300))
}

async fn new_repo(forge: &MemoryForge, default_branch: &str) -> RepositoryId {
    create_repo(forge, "acme", "service", default_branch).await
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

async fn spawn(handle: &skein::runtime::RuntimeHandle, daemon: &Daemon) -> String {
    let server = temper_engine::serve(
        handle,
        daemon,
        "127.0.0.1:0".parse().expect("loopback addr"),
    )
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

fn success_result(
    worker_id: &str,
    job_id: &str,
    repo: &str,
    branch_name: &str,
    summary: &str,
) -> JobResult {
    JobResult {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        job_id: job_id.to_string(),
        status: ResultStatus::Success,
        repos: vec![RepoOutcome {
            repo: repo.to_string(),
            branch: Branch {
                name: branch_name.to_string(),
                head_sha: "abc123".to_string(),
            },
        }],
        verdict: None,
        body: None,
        children: Vec::new(),
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
        repos: Vec::new(),
        verdict: None,
        body: None,
        children: Vec::new(),
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
        repos: Vec::new(),
        verdict: None,
        body: None,
        children: Vec::new(),
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
        repos: Vec::new(),
        verdict: Some(verdict.to_string()),
        body: body.map(str::to_string),
        children: Vec::new(),
        failure: None,
        summary: Some("triaged".to_string()),
        details: None,
    }
}

fn verdict_result_with_children(
    worker_id: &str,
    job_id: &str,
    verdict: &str,
    children: Vec<JobChild>,
) -> JobResult {
    let mut result = verdict_result(worker_id, job_id, verdict, None);
    result.children = children;
    result
}

fn job_child(slug: &str, title: &str, body: &str, labels: &[&str]) -> JobChild {
    JobChild {
        slug: slug.to_string(),
        title: title.to_string(),
        body: body.to_string(),
        labels: labels.iter().map(|label| (*label).to_string()).collect(),
        depends_on: Vec::new(),
        target_repo: None,
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
            artifact: None,
            workspace: None,
            action: None,
            checkout_capability: None,
            allowed_verdicts: Vec::new(),
        },
    )
}

fn writable_repo(repo: &str, branch: &str) -> WorkspaceRepo {
    let dir = repo.rsplit('/').next().unwrap_or(repo).to_string();
    WorkspaceRepo {
        repo: repo.to_string(),
        dir,
        access: RepoAccess::Writable,
        default_branch: "main".to_string(),
        base_branch: "main".to_string(),
        branch_hint: Some(branch.to_string()),
        depends_on: Vec::new(),
    }
}

fn coordinated_in_flight_job(
    primary_path: &str,
    number: ItemNumber,
    coordination_key: &str,
    repos: Vec<WorkspaceRepo>,
) -> InFlightJob {
    job_for_context(
        primary_path,
        number,
        "issue",
        JobContext {
            role: "engineer".to_string(),
            repo: primary_path.to_string(),
            queue: "code_ready".to_string(),
            artifact_kind: "code".to_string(),
            artifact: None,
            workspace: Some(WorkspaceManifest {
                coordination_key: coordination_key.to_string(),
                repos,
            }),
            action: None,
            checkout_capability: Some("writable".to_string()),
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
            artifact: None,
            workspace: None,
            action: Some("triage_intake".to_string()),
            checkout_capability: Some("read_only".to_string()),
            allowed_verdicts: vec![
                "ready_code".to_string(),
                "needs_design".to_string(),
                "needs_breakdown".to_string(),
            ],
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
            artifact: None,
            workspace: None,
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

async fn list_issues(forge: &MemoryForge, repo: &RepositoryId) -> Vec<Issue> {
    forge
        .list_issues(repo, IssueQuery::default())
        .await
        .expect("list issues succeeds")
}

async fn wait_for_issue_count(
    cx: &temper_io_engine::Cx,
    forge: &MemoryForge,
    repo: &RepositoryId,
    expected: usize,
) -> Vec<Issue> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let issues = list_issues(forge, repo).await;
        if issues.len() == expected {
            return issues;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {expected} issue(s), saw {}",
            issues.len()
        );
        temper_io_engine::runtime::sleep_for(cx, Duration::from_millis(10)).await;
    }
}

async fn assert_issue_count_stays(
    cx: &temper_io_engine::Cx,
    forge: &MemoryForge,
    repo: &RepositoryId,
    expected: usize,
) {
    let deadline = Instant::now() + Duration::from_millis(250);
    while Instant::now() < deadline {
        let issues = list_issues(forge, repo).await;
        assert_eq!(issues.len(), expected);
        temper_io_engine::runtime::sleep_for(cx, Duration::from_millis(10)).await;
    }
}

fn issue_by_slug<'a>(issues: &'a [Issue], slug: &str) -> &'a Issue {
    issues
        .iter()
        .find(|issue| {
            parse_metadata_block(&issue.body)
                .expect("issue metadata parses")
                .is_some_and(|metadata| {
                    metadata
                        .correlation_key
                        .as_deref()
                        .is_some_and(|key| key.contains(&format!("/child:{}:{slug}", slug.len())))
                })
        })
        .unwrap_or_else(|| panic!("issue for child slug {slug:?} exists"))
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
    assert!(
        !issue_labels(forge, repo, issue)
            .await
            .iter()
            .any(|label| label == "needs-human")
    );
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
    cx: &temper_io_engine::Cx,
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
        temper_io_engine::runtime::sleep_for(cx, Duration::from_millis(10)).await;
    }
}

async fn assert_pull_request_state_stays(
    cx: &temper_io_engine::Cx,
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
        temper_io_engine::runtime::sleep_for(cx, Duration::from_millis(10)).await;
    }
}

async fn wait_for_pull_request_count(
    cx: &temper_io_engine::Cx,
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
        temper_io_engine::runtime::sleep_for(cx, Duration::from_millis(10)).await;
    }
}

async fn assert_pull_request_count_stays(
    cx: &temper_io_engine::Cx,
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
        temper_io_engine::runtime::sleep_for(cx, Duration::from_millis(10)).await;
    }
}

async fn assign_review_job(
    handle: &skein::runtime::RuntimeHandle,
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
        temper_engine::system_clock(),
    ));
    let daemon = Daemon::with_applier(Arc::new(handle.clone()), applier);
    let url = spawn(handle, &daemon).await;
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
    temper_io_engine::block_on_with(move |cx, handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let pull_request = create_pull_request_needing_review(&forge, &repo).await;
        let (client, url, assignment) =
            assign_review_job(&handle, forge.clone(), &repo, pull_request).await;

        let result = verdict_result("worker-a", &assignment.job_id, "approve", None);
        assert_release(
            post_json(&client, &url, &WorkerProtocolMessage::Result(result)).await,
            "worker-a",
            &assignment.job_id,
        );

        let (labels, reviews) =
            wait_for_review_apply(&cx, &forge, &repo, pull_request, |labels, reviews| {
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
    temper_io_engine::block_on_with(move |cx, handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let pull_request = create_pull_request_needing_review(&forge, &repo).await;
        let (client, url, assignment) =
            assign_review_job(&handle, forge.clone(), &repo, pull_request).await;
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
            wait_for_review_apply(&cx, &forge, &repo, pull_request, |labels, reviews| {
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
        let replay_result =
            verdict_result("worker-a", &replay_job.job_id, "changes", Some(authored));
        ForgeApplier::new(forge.clone(), Arc::new(workflow()))
            .apply(replay_job, replay_result)
            .await;

        assert_pull_request_state_stays(&cx, &forge, &repo, pull_request, labels, 1).await;
    })
}

#[test]
fn review_verdict_escalate_adds_needs_architect_label() {
    temper_io_engine::block_on_with(move |cx, handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let pull_request = create_pull_request_needing_review(&forge, &repo).await;
        let (client, url, assignment) =
            assign_review_job(&handle, forge.clone(), &repo, pull_request).await;

        let result = verdict_result("worker-a", &assignment.job_id, "escalate", None);
        assert_release(
            post_json(&client, &url, &WorkerProtocolMessage::Result(result)).await,
            "worker-a",
            &assignment.job_id,
        );

        let (labels, reviews) =
            wait_for_review_apply(&cx, &forge, &repo, pull_request, |labels, reviews| {
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
    temper_io_engine::block_on_with(move |cx, handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let pull_request = create_pull_request_needing_review(&forge, &repo).await;
        let before = pull_request_labels_and_reviews(&forge, &repo, pull_request).await;
        let (client, url, assignment) =
            assign_review_job(&handle, forge.clone(), &repo, pull_request).await;

        let result = verdict_result("worker-a", &assignment.job_id, "merge_now", None);
        assert_release(
            post_json(&client, &url, &WorkerProtocolMessage::Result(result)).await,
            "worker-a",
            &assignment.job_id,
        );

        assert_pull_request_state_stays(&cx, &forge, &repo, pull_request, before.0, before.1.len())
            .await;
    })
}

#[test]
fn triage_verdict_success_rewrites_body_and_routes_labels_without_pr() {
    temper_io_engine::block_on_with(move |cx, handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_untriaged_intake_issue(&forge, &repo).await;
        let workflow = Arc::new(workflow());
        let compiled = workflow.compile();
        let applier = Arc::new(LeaseApplier::new(
            forge.clone(),
            policy(),
            "daemon-1",
            Arc::new(ForgeApplier::new(forge.clone(), workflow.clone())),
            temper_engine::system_clock(),
        ));
        let daemon = Daemon::with_applier(Arc::new(handle.clone()), applier);
        let url = spawn(&handle, &daemon).await;
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
        assert_eq!(
            context.allowed_verdicts,
            vec!["needs_breakdown", "needs_design", "ready_code"]
        );
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
            temper_io_engine::runtime::sleep_for(&cx, Duration::from_millis(10)).await;
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
    temper_io_engine::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_untriaged_intake_issue(&forge, &repo).await;
        let workflow = Arc::new(workflow());
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
fn breakdown_verdict_creates_children_across_repos() {
    temper_io_engine::block_on_with(move |cx, handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo_a = new_repo(&forge, "stable").await;
        let repo_b = create_repo(&forge, "acme", "web", "stable").await;
        let issue = create_untriaged_intake_issue(&forge, &repo_a).await;
        let workflow = Arc::new(workflow());
        let compiled = workflow.compile();
        let applier = Arc::new(LeaseApplier::new(
            forge.clone(),
            policy(),
            "daemon-1",
            Arc::new(ForgeApplier::new(forge.clone(), workflow.clone())),
            temper_engine::system_clock(),
        ));
        let daemon = Daemon::with_applier(Arc::new(handle.clone()), applier);
        let url = spawn(&handle, &daemon).await;
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
                    &repo_a,
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
        assert_eq!(
            context.allowed_verdicts,
            vec!["needs_breakdown", "needs_design", "ready_code"]
        );
        assert_eq!(context.checkout_capability.as_deref(), Some("read_only"));

        let mut web = job_child(
            "web-client",
            "Implement the web client",
            "Build the web client against the API schema.",
            &["code", "ready"],
        );
        web.depends_on = vec!["api-schema".to_string()];
        web.target_repo = Some("acme/web".to_string());
        let result = verdict_result_with_children(
            "worker-a",
            &assignment.job_id,
            "needs_breakdown",
            vec![
                job_child(
                    "api-schema",
                    "Define the API schema",
                    "Write the shared API schema.",
                    &["code", "ready"],
                ),
                web,
            ],
        );
        assert_release(
            post_json(&client, &url, &WorkerProtocolMessage::Result(result)).await,
            "worker-a",
            &assignment.job_id,
        );

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let labels = issue_labels(&forge, &repo_a, issue).await;
            if !has_label(&labels, "untriaged") && has_label(&labels, "epic") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for breakdown verdict apply, saw labels {:?}",
                labels
            );
            temper_io_engine::runtime::sleep_for(&cx, Duration::from_millis(10)).await;
        }

        let repo_a_issues = wait_for_issue_count(&cx, &forge, &repo_a, 2).await;
        let repo_b_issues = wait_for_issue_count(&cx, &forge, &repo_b, 1).await;
        let api_child = issue_by_slug(&repo_a_issues, "api-schema");
        let web_child = issue_by_slug(&repo_b_issues, "web-client");

        assert_eq!(
            api_child.labels,
            vec!["code".to_string(), "ready".to_string()]
        );
        let api_metadata = parse_metadata_block(&api_child.body)
            .expect("api child metadata parses")
            .expect("api child metadata exists");
        assert_eq!(api_metadata.parents, vec![ArtifactRef::same_repo(issue)]);

        assert_eq!(
            web_child.labels,
            vec!["code".to_string(), "ready".to_string()]
        );
        let web_metadata = parse_metadata_block(&web_child.body)
            .expect("web child metadata parses")
            .expect("web child metadata exists");
        assert_eq!(
            web_metadata.parents,
            vec![ArtifactRef::in_repo(repo_a.clone(), issue)]
        );
        let expected_web_correlation_key =
            global_child_correlation_key(&repo_a, issue, "web-client");
        assert_eq!(
            web_metadata.correlation_key.as_deref(),
            Some(expected_web_correlation_key.as_str())
        );
        assert_eq!(
            web_metadata.dependencies,
            vec![ArtifactRef::in_repo(repo_a.clone(), api_child.number)]
        );

        let parent = forge
            .get_issue_by_number(&repo_a, issue)
            .await
            .expect("parent reload succeeds")
            .expect("parent exists");
        let parent_metadata = parse_metadata_block(&parent.body)
            .expect("parent metadata parses")
            .expect("parent metadata exists");
        assert_eq!(
            parent_metadata.dependencies,
            vec![
                ArtifactRef::in_repo(repo_a.clone(), api_child.number),
                ArtifactRef::in_repo(repo_b.clone(), web_child.number),
            ]
        );
    })
}

#[test]
fn breakdown_verdict_replay_is_idempotent() {
    temper_io_engine::block_on_with(move |cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo_a = new_repo(&forge, "stable").await;
        let repo_b = create_repo(&forge, "acme", "web", "stable").await;
        let issue = create_untriaged_intake_issue(&forge, &repo_a).await;
        let applier = ForgeApplier::new(forge.clone(), Arc::new(workflow()));
        let job = triage_in_flight_job("acme/service", issue);

        let mut web = job_child(
            "web-client",
            "Implement the web client",
            "Build the web client against the API schema.",
            &["code", "ready"],
        );
        web.depends_on = vec!["api-schema".to_string()];
        web.target_repo = Some("acme/web".to_string());
        let result = verdict_result_with_children(
            "worker-a",
            &job.job_id,
            "needs_breakdown",
            vec![
                job_child(
                    "api-schema",
                    "Define the API schema",
                    "Write the shared API schema.",
                    &["code", "ready"],
                ),
                web,
            ],
        );

        applier.apply(job.clone(), result.clone()).await;
        let repo_a_issues = list_issues(&forge, &repo_a).await;
        let repo_b_issues = list_issues(&forge, &repo_b).await;
        assert_eq!(repo_a_issues.len(), 2);
        assert_eq!(repo_b_issues.len(), 1);
        let api_child = issue_by_slug(&repo_a_issues, "api-schema");
        let web_child = issue_by_slug(&repo_b_issues, "web-client");
        let parent = forge
            .get_issue_by_number(&repo_a, issue)
            .await
            .expect("parent reload succeeds")
            .expect("parent exists");
        let parent_dependencies = parse_metadata_block(&parent.body)
            .expect("parent metadata parses")
            .expect("parent metadata exists")
            .dependencies;
        assert_eq!(
            parent_dependencies,
            vec![
                ArtifactRef::in_repo(repo_a.clone(), api_child.number),
                ArtifactRef::in_repo(repo_b.clone(), web_child.number),
            ]
        );

        applier.apply(job, result).await;

        assert_issue_count_stays(&cx, &forge, &repo_a, 2).await;
        assert_issue_count_stays(&cx, &forge, &repo_b, 1).await;
        let parent = forge
            .get_issue_by_number(&repo_a, issue)
            .await
            .expect("parent reload succeeds")
            .expect("parent exists");
        let after_replay_dependencies = parse_metadata_block(&parent.body)
            .expect("parent metadata parses")
            .expect("parent metadata exists")
            .dependencies;
        assert_eq!(after_replay_dependencies, parent_dependencies);
    })
}

#[test]
fn children_without_create_issues_effect_are_ignored() {
    temper_io_engine::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_untriaged_intake_issue(&forge, &repo).await;
        let applier = ForgeApplier::new(forge.clone(), Arc::new(workflow()));
        let job = triage_in_flight_job("acme/service", issue);
        let mut result = verdict_result(
            "worker-a",
            &job.job_id,
            "ready_code",
            Some("rewritten spec"),
        );
        result.children = vec![job_child(
            "stray-child",
            "Do not create me",
            "This child is not bound by the ready_code route.",
            &["code", "ready"],
        )];

        applier.apply(job, result).await;

        let (body, labels) = issue_body_and_labels(&forge, &repo, issue).await;
        assert_eq!(body, "rewritten spec");
        assert!(!has_label(&labels, "untriaged"));
        assert!(has_label(&labels, "code"));
        assert!(has_label(&labels, "ready"));
        assert_eq!(list_issues(&forge, &repo).await.len(), 1);
    })
}

#[test]
fn unresolvable_child_target_repo_drops_apply() {
    temper_io_engine::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo_a = new_repo(&forge, "stable").await;
        let repo_b = create_repo(&forge, "acme", "web", "stable").await;
        let issue = create_untriaged_intake_issue(&forge, &repo_a).await;
        let applier = ForgeApplier::new(forge.clone(), Arc::new(workflow()));
        let job = triage_in_flight_job("acme/service", issue);
        let mut child = job_child(
            "api-schema",
            "Define the API schema",
            "Write the shared API schema.",
            &["code", "ready"],
        );
        child.target_repo = Some("nobody/nowhere".to_string());
        let result =
            verdict_result_with_children("worker-a", &job.job_id, "needs_breakdown", vec![child]);

        applier.apply(job, result).await;

        let (body, labels) = issue_body_and_labels(&forge, &repo_a, issue).await;
        assert_eq!(body, "rough user request");
        assert_eq!(labels, vec!["untriaged".to_string()]);
        assert_eq!(list_issues(&forge, &repo_a).await.len(), 1);
        assert!(list_issues(&forge, &repo_b).await.is_empty());
    })
}

#[test]
fn undeclared_verdict_does_not_mutate_issue() {
    temper_io_engine::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_untriaged_intake_issue(&forge, &repo).await;
        let workflow = Arc::new(workflow());
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
    temper_io_engine::block_on_with(move |cx, handle| async move {
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
            temper_engine::system_clock(),
        ));
        let daemon = Daemon::with_applier(Arc::new(handle.clone()), applier);
        let url = spawn(&handle, &daemon).await;
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
        let posted_result = success_result(
            "worker-a",
            &assignment.job_id,
            &assignment.repo,
            &branch_name,
            summary,
        );
        assert_release(
            post_json(&client, &url, &WorkerProtocolMessage::Result(posted_result)).await,
            "worker-a",
            &assignment.job_id,
        );

        let pulls = wait_for_pull_request_count(&cx, &forge, &repo, 1).await;
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
        let replay_result = success_result(
            "worker-a",
            &assignment.job_id,
            &assignment.repo,
            &branch_name,
            summary,
        );
        ForgeApplier::new(forge.clone(), workflow.clone())
            .apply(replay_job, replay_result)
            .await;
        assert_pull_request_count_stays(&cx, &forge, &repo, 1).await;
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

        assert_pull_request_count_stays(&cx, &forge, &repo, 1).await;
    })
}

#[test]
fn coordinated_result_opens_one_pull_request_per_writable_repo() {
    temper_io_engine::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        // Primary (home of the coordinating issue) + a second writable repo.
        let primary = create_repo(&forge, "acme", "service", "main").await;
        let secondary = create_repo(&forge, "acme", "lib", "main").await;
        let issue = create_ready_issue(&forge, &primary).await;
        let workflow = Arc::new(workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow.clone());

        let coordination_key = format!("coord-for-code-{}", issue.get());
        let branch = format!("agent/{coordination_key}");
        let job = coordinated_in_flight_job(
            "acme/service",
            issue,
            &coordination_key,
            vec![
                writable_repo("acme/service", &branch),
                writable_repo("acme/lib", &branch),
            ],
        );
        let result = JobResult {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: "worker-a".to_string(),
            job_id: job.job_id.clone(),
            status: ResultStatus::Success,
            repos: vec![
                RepoOutcome {
                    repo: "acme/service".to_string(),
                    branch: Branch {
                        name: branch.clone(),
                        head_sha: "aaa111".to_string(),
                    },
                },
                RepoOutcome {
                    repo: "acme/lib".to_string(),
                    branch: Branch {
                        name: branch.clone(),
                        head_sha: "bbb222".to_string(),
                    },
                },
            ],
            verdict: None,
            body: None,
            children: Vec::new(),
            failure: None,
            summary: Some("coordinated cross-repo change".to_string()),
            details: None,
        };

        applier.apply(job, result).await;

        // The primary repo's PR links back to the coordinating issue with a
        // bare same-repo ref, and carries the shared coordination key.
        let primary_pulls = forge
            .list_pull_requests(&primary, PullRequestQuery::default())
            .await
            .expect("list primary pull requests");
        assert_eq!(primary_pulls.len(), 1, "one PR opened in the primary repo");
        let primary_pull = &primary_pulls[0];
        assert_eq!(primary_pull.source.branch, branch);
        assert_eq!(primary_pull.target.branch, "main");
        let primary_meta = parse_metadata_block(&primary_pull.body)
            .expect("primary PR metadata parses")
            .expect("primary PR metadata exists");
        assert_eq!(primary_meta.parents, vec![ArtifactRef::same_repo(issue)]);
        assert_eq!(
            primary_meta.correlation_key.as_deref(),
            Some(coordination_key.as_str())
        );

        // The secondary repo's PR links to the SAME coordinating issue, but
        // repo-qualified to the primary repo — the cross-repo backref.
        let secondary_pulls = forge
            .list_pull_requests(&secondary, PullRequestQuery::default())
            .await
            .expect("list secondary pull requests");
        assert_eq!(
            secondary_pulls.len(),
            1,
            "one PR opened in the secondary repo"
        );
        let secondary_pull = &secondary_pulls[0];
        assert_eq!(secondary_pull.source.branch, branch);
        assert_eq!(secondary_pull.target.repository_id, secondary);
        let secondary_meta = parse_metadata_block(&secondary_pull.body)
            .expect("secondary PR metadata parses")
            .expect("secondary PR metadata exists");
        assert_eq!(
            secondary_meta.parents,
            vec![ArtifactRef::in_repo(primary.clone(), issue)]
        );
        assert_eq!(
            secondary_meta.correlation_key.as_deref(),
            Some(coordination_key.as_str())
        );
    })
}

#[test]
fn peer_owned_lease_prevents_forge_apply_and_preserves_peer_metadata() {
    temper_io_engine::block_on_with(move |_cx, _handle| async move {
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
            temper_engine::system_clock(),
        );
        let job = in_flight_job("acme/service", issue);
        let result = success_result(
            "worker-a",
            &job.job_id,
            "acme/service",
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
    temper_io_engine::block_on_with(move |_cx, _handle| async move {
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
    temper_io_engine::block_on_with(move |_cx, _handle| async move {
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
    temper_io_engine::block_on_with(move |_cx, _handle| async move {
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
    temper_io_engine::block_on_with(move |_cx, _handle| async move {
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
    temper_io_engine::block_on_with(move |_cx, _handle| async move {
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
    temper_io_engine::block_on_with(move |_cx, _handle| async move {
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
    temper_io_engine::block_on_with(move |_cx, _handle| async move {
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
        assert!(
            !issue_labels(&forge, &repo, issue)
                .await
                .iter()
                .any(|label| label == "needs-human")
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

// ---------------------------------------------------------------------------
// Step-progress checkpoints (worker → daemon → forge relay, phase 6a).
// ---------------------------------------------------------------------------

#[test]
fn progress_checkpoints_are_recorded_once_per_step() {
    use temper_worker_protocol::JobProgress;

    temper_io_engine::block_on(async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let number = create_ready_issue(&forge, &repo).await;
        let applier = ForgeApplier::new(forge.clone(), Arc::new(workflow()));

        let job = InFlightJob {
            job_id: "acme/service/issue-1/engineer/code_ready".to_string(),
            role: "engineer".to_string(),
            repo: "acme/service".to_string(),
            artifact: Artifact {
                item: json!(number.get()),
                kind: "issue".to_string(),
            },
            job_payload: json!({ "correlation_key": "pr-for-code-9" }),
        };
        let progress = |step: u32, state: &str| JobProgress {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: "worker-a".to_string(),
            correlation_key: "pr-for-code-9".to_string(),
            step,
            status: "write failing test".to_string(),
            state: state.to_string(),
            pushed_sha: Some("0123456789abcdef0123456789abcdef01234567".to_string()),
            note: None,
        };

        applier
            .apply_progress(job.clone(), progress(1, "done"))
            .await;
        // Re-delivery of the same (correlation_key, step, state) is a no-op.
        applier
            .apply_progress(job.clone(), progress(1, "done"))
            .await;
        // A different step (or phase) appends its own checkpoint.
        applier
            .apply_progress(job.clone(), progress(2, "started"))
            .await;

        let issue = forge
            .get_issue_by_number(&repo, number)
            .await
            .expect("issue lookup succeeds")
            .expect("issue exists");
        let comments = forge
            .list_issue_comments(&issue.id)
            .await
            .expect("comments list");
        let progress_comments: Vec<_> = comments
            .iter()
            .filter(|comment| comment.body.contains("temper-progress"))
            .collect();
        assert_eq!(
            progress_comments.len(),
            2,
            "duplicate delivery must not duplicate forge state: {progress_comments:?}"
        );
        assert!(
            progress_comments[0]
                .body
                .contains("- [x] step 1: write failing test (engineer, pushed 0123456789ab"),
            "checkpoint line renders: {}",
            progress_comments[0].body
        );
        assert!(
            progress_comments[1].body.contains("- [ ] step 2:"),
            "started phase renders unticked: {}",
            progress_comments[1].body
        );
    })
}
