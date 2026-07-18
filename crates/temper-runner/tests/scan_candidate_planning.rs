//! Regression tests for queue-derived scan candidate queries.

mod support;

use chrono::{DateTime, Utc};
use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use support::{CountedForgeOp, CountingForge};
use temper_forge::{
    BranchRef, CandidateLabelSelection, CandidateLifecycle, CreateIssue, CreatePullRequest,
    CreatePullRequestReview, CreateRepository, Forge, IssueCandidateQuery, IssueState,
    ItemListDetails, ItemNumber, MergeMethod, MergePullRequest, PullRequestCandidateQuery,
    PullRequestUpdateState, RepositoryId, RequestReviewers, ReviewDecision, UpdateIssue,
    UpdatePullRequest, UserId,
};
use temper_forge_memory::MemoryForge;
use temper_runner::{
    ScanMode, WorkItem, candidate_query_plan, scan_role, scan_role_audit, scan_role_wake,
};
use temper_workflow::{ArtifactKindId, ArtifactSource, QueueId, RawWorkflowSpec, RoleId};

const REFERENCE_FIXTURE: &str =
    include_str!("../../temper-workflow/fixtures/reference-delivery.json");
const BASIC_FIXTURE: &str = include_str!("../../temper-workflow/fixtures/basic-delivery.json");

const PLANNER_FIXTURE: &str = r#"
{
  "name": "candidate-planning",
  "roles": [
    { "id": "engineer", "queues": ["branchy", "review_signal"] }
  ],
  "labels": [
    { "id": "code" },
    { "id": "implementation" },
    { "id": "ready" },
    { "id": "urgent" },
    { "id": "bug" },
    { "id": "landed" }
  ],
  "artifact_kinds": [
    { "id": "code", "target": "issue", "identifying_labels": ["code"] },
    { "id": "implementation_pr", "target": "pull_request", "identifying_labels": ["implementation"] }
  ],
  "queues": [
    {
      "id": "branchy",
      "artifact": "code",
      "labels": ["ready"],
      "any_of": [{ "labels": ["urgent"] }, { "labels": ["bug"] }]
    },
    {
      "id": "review_signal",
      "artifact": "implementation_pr",
      "condition": { "kind": "review_changes_requested" }
    }
  ]
}
"#;

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
                branch: format!("feature-{}", labels.join("-")),
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

fn close_pr(forge: &MemoryForge, repo: &RepositoryId, number: ItemNumber) {
    let pull_request = block_on(forge.get_pull_request_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("pull request exists");
    block_on(forge.update_pull_request(
        &pull_request.id,
        UpdatePullRequest {
            state: Some(PullRequestUpdateState::Closed),
            ..UpdatePullRequest::default()
        },
    ))
    .expect("pull request closes");
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

fn closed_issue_queries_have_labels(queries: &[IssueCandidateQuery]) -> bool {
    queries
        .iter()
        .filter(|query| query.lifecycle == CandidateLifecycle::Terminal)
        .all(|query| matches!(query.labels, CandidateLabelSelection::AnyOf(ref labels) if !labels.is_empty()))
}

fn closed_pull_request_queries_have_labels(queries: &[PullRequestCandidateQuery]) -> bool {
    queries
        .iter()
        .filter(|query| query.lifecycle == CandidateLifecycle::Terminal)
        .all(|query| matches!(query.labels, CandidateLabelSelection::AnyOf(ref labels) if !labels.is_empty()))
}

fn has_issue_query(
    queries: &[IssueCandidateQuery],
    lifecycle: CandidateLifecycle,
    labels: &[&str],
) -> bool {
    queries
        .iter()
        .any(|query| query.lifecycle == lifecycle && candidate_labels_match(&query.labels, labels))
}

fn has_pull_request_query(
    queries: &[PullRequestCandidateQuery],
    lifecycle: CandidateLifecycle,
    labels: &[&str],
) -> bool {
    queries
        .iter()
        .any(|query| query.lifecycle == lifecycle && candidate_labels_match(&query.labels, labels))
}

fn candidate_labels_match(selection: &CandidateLabelSelection, labels: &[&str]) -> bool {
    if labels.is_empty() {
        return matches!(selection, CandidateLabelSelection::Unfiltered);
    }
    matches!(selection, CandidateLabelSelection::AnyOf(actual) if labels.iter().all(|expected| actual.iter().any(|label| label == expected)))
}

#[path = "scan_candidate_planning/plan_queries.rs"]
mod plan_queries;
#[path = "scan_candidate_planning/role_scan_recovery.rs"]
mod role_scan_recovery;
