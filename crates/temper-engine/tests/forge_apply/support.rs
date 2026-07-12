// SPDX-License-Identifier: MPL-2.0

pub(crate) use std::{sync::Arc, time::Duration};

pub(crate) use serde_json::json;
pub(crate) use std::time::Instant;
pub(crate) use temper_engine::{
    Daemon, ForgeApplier, JobContext, LeaseApplier, ResultApplier, RoleFeedMode,
};
pub(crate) use temper_forge::{
    BranchRef, CreateIssue, CreatePullRequest, CreateRepository, Forge, Issue, IssueQuery,
    ItemNumber, PullRequest, PullRequestQuery, PullRequestReview, RepositoryId, ReviewDecision,
    UpdateIssue, UpdatePullRequest, User, UserId,
};
pub(crate) use temper_forge_memory::MemoryForge;
pub(crate) use temper_protocol_worker::{
    Artifact, Assign, Branch, Capability, Capacity, Failure, FailureClass, JobChild, JobResult,
    Poll, Register, ReleaseDisposition, RepoAccess, RepoOutcome, ResultStatus,
    WORKER_PROTOCOL_VERSION, WorkerProtocolMessage, WorkspaceManifest, WorkspaceRepo,
};
pub(crate) use temper_worker_registry::InFlightJob;
pub(crate) use temper_workflow::{
    ArtifactKindId, ArtifactRef, ArtifactSource, ArtifactTarget, DurableAssignment, LeaseManager,
    LeasePolicy, RawArtifactKind, RawLabel, RawWorkflowSpec, RoleId, ValidatedWorkflow,
    WorkflowMetadata, global_child_correlation_key, parse_metadata_block, render_metadata_block,
};

const REFERENCE_FIXTURE: &str =
    include_str!("../../../temper-workflow/fixtures/reference-delivery.json");

pub(crate) fn workflow() -> ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(REFERENCE_FIXTURE).expect("workflow parses");
    spec.validate().expect("workflow validates")
}

pub(crate) fn workflow_with_plan_kind() -> ValidatedWorkflow {
    let mut spec: RawWorkflowSpec =
        serde_json::from_str(REFERENCE_FIXTURE).expect("workflow parses");
    spec.labels.push(RawLabel {
        id: "plan".to_string(),
        description: Some("identifies an architect-authored plan issue".to_string()),
    });
    spec.artifact_kinds.push(RawArtifactKind {
        id: "plan".to_string(),
        target: ArtifactTarget::Issue,
        identifying_labels: vec!["plan".to_string()],
        initial_labels: Vec::new(),
    });
    spec.queues.push(
        serde_json::from_value(json!({
            "id": "test_plan_ready",
            "artifact": "plan",
            "actions": [{"role": "architect", "action": "handle_test_plan"}]
        }))
        .expect("test plan queue parses"),
    );
    spec.transitions.push(
        serde_json::from_value(json!({
            "id": "handle_test_plan",
            "artifact": "plan",
            "roles": ["architect"]
        }))
        .expect("test plan transition parses"),
    );
    spec.roles
        .iter_mut()
        .find(|role| role.id == "architect")
        .expect("reference workflow has architect")
        .queues
        .push("test_plan_ready".to_string());
    spec.validate().expect("workflow with plan validates")
}

pub(crate) fn ts(value: &str) -> chrono::DateTime<chrono::Utc> {
    value.parse().expect("valid RFC 3339 timestamp")
}

pub(crate) fn policy() -> LeasePolicy {
    LeasePolicy::new(chrono::Duration::seconds(300))
}

pub(crate) fn role_user(role: &str) -> User {
    User {
        id: UserId::new(role),
        handle: role.to_string(),
        display_name: None,
        email: None,
    }
}

pub(crate) async fn new_repo(forge: &MemoryForge, default_branch: &str) -> RepositoryId {
    create_repo(forge, "acme", "service", default_branch).await
}

pub(crate) async fn create_repo(
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

pub(crate) async fn create_ready_issue(forge: &MemoryForge, repo: &RepositoryId) -> ItemNumber {
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

pub(crate) async fn create_untriaged_intake_issue(
    forge: &MemoryForge,
    repo: &RepositoryId,
) -> ItemNumber {
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

pub(crate) async fn create_pull_request_needing_review(
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

pub(crate) async fn issue_body_and_labels(
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

pub(crate) async fn spawn(handle: &skein::runtime::RuntimeHandle, daemon: &Daemon) -> String {
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

pub(crate) fn register(worker_id: &str, role: &str, repo: &str) -> WorkerProtocolMessage {
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
        worker_pool: None,
        labels: None,
    })
}

pub(crate) fn poll(worker_id: &str) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Poll(Poll {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        free_capacity: 1,
        max_wait_ms: Some(30_000),
    })
}

pub(crate) fn success_result(
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
        title: None,
        body: None,
        children: Vec::new(),
        failure: None,
        summary: Some(summary.to_string()),
        details: Some(json!({"note":"fake worker result"})),
    }
}

pub(crate) fn failure_result(
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
        title: None,
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

pub(crate) fn permanent_failure_result(worker_id: &str, job_id: &str) -> JobResult {
    failure_result(
        worker_id,
        job_id,
        Some(FailureClass::Permanent),
        "not implemented",
    )
}

pub(crate) fn success_without_branch(worker_id: &str, job_id: &str) -> JobResult {
    JobResult {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        job_id: job_id.to_string(),
        status: ResultStatus::Success,
        repos: Vec::new(),
        verdict: None,
        title: None,
        body: None,
        children: Vec::new(),
        failure: None,
        summary: Some("done".to_string()),
        details: None,
    }
}

pub(crate) fn verdict_result(
    worker_id: &str,
    job_id: &str,
    verdict: &str,
    body: Option<&str>,
) -> JobResult {
    JobResult {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        job_id: job_id.to_string(),
        status: ResultStatus::Success,
        repos: Vec::new(),
        verdict: Some(verdict.to_string()),
        title: None,
        body: body.map(str::to_string),
        children: Vec::new(),
        failure: None,
        summary: Some("triaged".to_string()),
        details: None,
    }
}

pub(crate) fn verdict_result_with_children(
    worker_id: &str,
    job_id: &str,
    verdict: &str,
    children: Vec<JobChild>,
) -> JobResult {
    let mut result = verdict_result(worker_id, job_id, verdict, None);
    result.children = children;
    result
}

pub(crate) fn job_child(slug: &str, title: &str, body: &str, labels: &[&str]) -> JobChild {
    JobChild {
        slug: slug.to_string(),
        title: title.to_string(),
        body: body.to_string(),
        kind: None,
        labels: labels.iter().map(|label| (*label).to_string()).collect(),
        depends_on: Vec::new(),
        target_repo: None,
    }
}

pub(crate) fn in_flight_job(repo_path: &str, number: ItemNumber) -> InFlightJob {
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
            verdict_contracts: Default::default(),
            source_metadata: Default::default(),
            guidance: None,
            pull_request_freshness: None,
        },
    )
}

pub(crate) fn open_pr_in_flight_job(repo_path: &str, number: ItemNumber) -> InFlightJob {
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
            action: Some("open_pr".to_string()),
            checkout_capability: Some("writable".to_string()),
            allowed_verdicts: vec!["needs_architect".to_string(), "needs_human".to_string()],
            verdict_contracts: Default::default(),
            source_metadata: Default::default(),
            guidance: None,
            pull_request_freshness: None,
        },
    )
}

pub(crate) fn writable_repo(repo: &str, branch: &str) -> WorkspaceRepo {
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

pub(crate) fn coordinated_in_flight_job(
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
            verdict_contracts: Default::default(),
            source_metadata: Default::default(),
            guidance: None,
            pull_request_freshness: None,
        },
    )
}

pub(crate) fn triage_in_flight_job(repo_path: &str, number: ItemNumber) -> InFlightJob {
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
            verdict_contracts: Default::default(),
            source_metadata: Default::default(),
            guidance: None,
            pull_request_freshness: None,
        },
    )
}

pub(crate) fn review_in_flight_job(repo_path: &str, number: ItemNumber) -> InFlightJob {
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
            verdict_contracts: Default::default(),
            source_metadata: Default::default(),
            guidance: None,
            pull_request_freshness: None,
        },
    )
}

pub(crate) fn job_for_context(
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

pub(crate) async fn post(
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

pub(crate) async fn post_json(
    client: &temper_engine_io::http::JsonClient,
    url: &str,
    msg: &WorkerProtocolMessage,
) -> WorkerProtocolMessage {
    let response = post(client, url, msg).await;
    assert_eq!(response.status, 200);
    serde_json::from_slice(&response.body).expect("protocol response json")
}

pub(crate) fn assert_release(msg: WorkerProtocolMessage, worker_id: &str, job_id: &str) {
    match msg {
        WorkerProtocolMessage::Release(release) => {
            assert_eq!(release.worker_id, worker_id);
            assert_eq!(release.job_id, job_id);
            assert_eq!(release.disposition, ReleaseDisposition::Accepted);
        }
        other => panic!("expected release, got {other:?}"),
    }
}

pub(crate) async fn poll_assignment_for_role(
    client: &temper_engine_io::http::JsonClient,
    url: &str,
    worker_id: &str,
    expected_role: &str,
    expected_artifact_kind: &str,
    number: ItemNumber,
) -> temper_protocol_worker::Assign {
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

pub(crate) async fn poll_assignment(
    client: &temper_engine_io::http::JsonClient,
    url: &str,
    worker_id: &str,
    issue: ItemNumber,
) -> temper_protocol_worker::Assign {
    poll_assignment_for_role(client, url, worker_id, "engineer", "issue", issue).await
}

pub(crate) async fn poll_review_assignment(
    client: &temper_engine_io::http::JsonClient,
    url: &str,
    worker_id: &str,
    pull_request: ItemNumber,
) -> temper_protocol_worker::Assign {
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

pub(crate) fn assert_durable_assignment(issue: &Issue, assignment: &Assign) {
    assert_eq!(
        issue.labels,
        vec!["code".to_string(), "in-progress".to_string()]
    );
    assert_eq!(issue.assignees, vec![UserId::new("engineer")]);
    let metadata = parse_metadata_block(&issue.body)
        .expect("assignment metadata parses")
        .expect("assignment metadata exists");
    let durable = metadata
        .assignment
        .expect("poll does not return before the durable assignment exists");
    assert_eq!(durable.job_id.as_deref(), Some(assignment.job_id.as_str()));
    assert_eq!(durable.worker_id.as_deref(), Some("worker-a"));
    assert!(durable.daemon_boot_id.is_some());
    assert!(metadata.lease.is_some());
}

pub(crate) fn assert_durable_assignment_released(issue: &Issue) {
    let metadata = parse_metadata_block(&issue.body)
        .expect("completed metadata parses")
        .unwrap_or_default();
    assert!(metadata.assignment.is_none());
    assert!(metadata.lease.is_none());
}
