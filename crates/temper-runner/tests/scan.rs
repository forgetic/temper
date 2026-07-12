//! Integration tests for Forge-backed runner scans.

mod support;

use chrono::{DateTime, Utc};
use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use support::{CountedForgeOp, CountingForge};
use temper_forge::{
    BranchRef, CiJob, CiJobConclusion, CiJobId, CiJobStatus, CreateIssue, CreatePullRequest,
    CreatePullRequestReview, CreateRepository, Forge, IssueState, ItemListDetails, ItemNumber,
    MergeMethod, MergePullRequest, RepositoryId, RequestReviewers, ReviewDecision, UpdateIssue,
    UserId,
};
use temper_forge_memory::MemoryForge;
use temper_runner::{AutomatedWorkItem, WorkItem, scan, scan_automated_queues, scan_role};
use temper_workflow::{
    ArtifactKindId, ArtifactSource, QueueId, RawWorkflowSpec, RoleId, WorkflowMetadata,
    render_metadata_block,
};

const FIXTURE: &str = include_str!("../../temper-workflow/fixtures/reference-delivery.json");

const REVIEW_ONLY_FIXTURE: &str = r#"
{
  "name": "review-only",
  "roles": [
    { "id": "review_watcher", "queues": ["review_changes"] }
  ],
  "labels": [
    { "id": "implementation" }
  ],
  "artifact_kinds": [
    {
      "id": "implementation_pr",
      "target": "pull_request",
      "identifying_labels": ["implementation"]
    }
  ],
  "queues": [
    {
      "id": "review_changes",
      "artifact": "implementation_pr",
      "condition": { "kind": "review_changes_requested" }
    }
  ]
}
"#;

// A workflow that models raw human intake: a default (catch-all) `intake` issue
// kind with no identifying labels, and a mechanical queue with no label filter
// that stamps any unlabeled issue `untriaged` via `mark_untriaged`. This is the
// first step of the target intake flow (issue #35): an unlabeled human issue is
// admitted by the default kind and mechanically transitioned to `untriaged`.
const INTAKE_DEFAULT_FIXTURE: &str = r#"
{
  "name": "intake-default",
  "roles": [
    { "id": "mechanical" }
  ],
  "labels": [
    { "id": "untriaged" },
    { "id": "code" }
  ],
  "artifact_kinds": [
    { "id": "intake", "target": "issue", "identifying_labels": [] },
    { "id": "code", "target": "issue", "identifying_labels": ["code"] }
  ],
  "transitions": [
    {
      "id": "mark_untriaged",
      "artifact": "intake",
      "roles": ["mechanical"],
      "effects": [
        { "kind": "add_label", "label": "untriaged" }
      ]
    }
  ],
  "queues": [
    {
      "id": "raw_intake",
      "artifact": "intake",
      "automation": {
        "actor": "mechanical",
        "transition": "mark_untriaged"
      }
    }
  ]
}
"#;

const DEPENDENCY_QUEUE_FIXTURE: &str = r#"
{
  "name": "dependency-queue",
  "roles": [
    { "id": "dependency_watcher", "queues": ["dependencies_clear"] }
  ],
  "labels": [
    { "id": "code" },
    { "id": "blocked" }
  ],
  "artifact_kinds": [
    {
      "id": "code",
      "target": "issue",
      "identifying_labels": ["code"]
    }
  ],
  "relations": [
    { "kind": "dependency", "source": "code", "target": "code" }
  ],
  "queues": [
    {
      "id": "dependencies_clear",
      "artifact": "code",
      "labels": ["blocked"],
      "condition": { "kind": "dependencies_resolved" }
    }
  ]
}
"#;

struct NoopWake;

impl Wake for NoopWake {
    fn wake(self: Arc<Self>) {}
}

fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(NoopWake));
    let mut context = Context::from_waker(&waker);
    let mut future = Box::pin(future);
    match Future::poll(future.as_mut(), &mut context) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("in-memory forge futures should not park in tests"),
    }
}

fn ts(value: &str) -> DateTime<Utc> {
    value.parse().expect("valid RFC 3339 timestamp")
}

fn workflow() -> temper_workflow::ValidatedWorkflow {
    workflow_from_json(FIXTURE)
}

fn workflow_from_json(json: &str) -> temper_workflow::ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(json).expect("workflow parses");
    spec.validate().expect("workflow validates")
}

fn new_repo(forge: &MemoryForge) -> RepositoryId {
    block_on(forge.create_repository(CreateRepository {
        owner: "acme".into(),
        name: "service".into(),
        default_branch: "main".into(),
        description: None,
    }))
    .expect("repository is created")
    .id
}

fn create_issue(forge: &MemoryForge, repo: &RepositoryId, labels: &[&str]) -> ItemNumber {
    block_on(forge.create_issue(
        repo,
        CreateIssue {
            title: "issue".into(),
            body: String::new(),
            labels: labels.iter().map(|label| (*label).to_string()).collect(),
            assignees: Vec::new(),
        },
    ))
    .expect("issue is created")
    .number
}

fn create_pr(forge: &MemoryForge, repo: &RepositoryId, labels: &[&str]) -> ItemNumber {
    block_on(forge.create_pull_request(
        repo,
        CreatePullRequest {
            title: "implementation".into(),
            body: String::new(),
            source: BranchRef {
                repository_id: repo.clone(),
                branch: "feature".into(),
            },
            target: BranchRef {
                repository_id: repo.clone(),
                branch: "main".into(),
            },
            labels: labels.iter().map(|label| (*label).to_string()).collect(),
            assignees: Vec::<UserId>::new(),
        },
    ))
    .expect("pull request is created")
    .number
}

fn seed_ci(
    forge: &MemoryForge,
    repo: &RepositoryId,
    number: ItemNumber,
    conclusion: CiJobConclusion,
) {
    let pull_request = block_on(forge.get_pull_request_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("pull request exists");
    forge.seed_ci_jobs(
        repo,
        vec![CiJob {
            id: CiJobId::new(format!("ci-{}-{}", repo.as_str(), number.get())),
            repo_id: repo.clone(),
            pull_request_id: Some(pull_request.id),
            commit_sha: pull_request.head_sha.unwrap_or_default(),
            name: "ci".into(),
            status: CiJobStatus::Completed,
            conclusion: Some(conclusion),
            url: None,
            created_at: ts("2026-05-29T00:00:00Z"),
            started_at: Some(ts("2026-05-29T00:00:30Z")),
            completed_at: Some(ts("2026-05-29T00:01:00Z")),
            updated_at: ts("2026-05-29T00:01:00Z"),
        }],
    );
}

fn merge_pr(forge: &MemoryForge, repo: &RepositoryId, number: ItemNumber) {
    let pull_request = block_on(forge.get_pull_request_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("pull request exists");
    block_on(forge.merge_pull_request(
        &pull_request.id,
        MergePullRequest {
            method: MergeMethod::Squash,
            commit_title: None,
            commit_body: None,
            delete_source_branch: false,
        },
    ))
    .expect("pull request merges");
}

fn submit_review(
    forge: &MemoryForge,
    repo: &RepositoryId,
    number: ItemNumber,
    decision: ReviewDecision,
) {
    let pull_request = block_on(forge.get_pull_request_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("pull request exists");
    block_on(forge.request_pull_request_reviewers(
        &pull_request.id,
        RequestReviewers {
            reviewers: vec![UserId::new("user-1")],
        },
    ))
    .expect("reviewer requested");
    block_on(forge.submit_pull_request_review(
        &pull_request.id,
        CreatePullRequestReview {
            decision,
            body: None,
        },
    ))
    .expect("review submitted");
}

fn close_issue(forge: &MemoryForge, repo: &RepositoryId, number: ItemNumber) {
    let issue = block_on(forge.get_issue_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("issue exists");
    block_on(forge.update_issue(
        &issue.id,
        UpdateIssue {
            state: Some(IssueState::Closed),
            ..UpdateIssue::default()
        },
    ))
    .expect("issue closes");
}

fn add_issue_dependency(
    forge: &MemoryForge,
    repo: &RepositoryId,
    source: ItemNumber,
    target: ItemNumber,
) {
    let issue = block_on(forge.get_issue_by_number(repo, source))
        .expect("lookup succeeds")
        .expect("issue exists");
    block_on(forge.add_issue_dependency(&issue.id, target)).expect("dependency link added");
}

fn scan_repo(forge: &MemoryForge, repo: &RepositoryId) -> Vec<WorkItem> {
    let workflow = workflow();
    let compiled = workflow.compile();
    block_on(scan(
        forge,
        repo,
        &workflow,
        &compiled,
        ts("2026-05-29T00:00:00Z"),
    ))
    .expect("scan succeeds")
}

#[path = "scan/basic.rs"]
mod basic;
#[path = "scan/gated_queues.rs"]
mod gated_queues;
#[path = "scan/intake.rs"]
mod intake;
