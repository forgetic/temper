//! Regression tests for queue-derived scan candidate queries.

mod support;

use chrono::{DateTime, Utc};
use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use support::{CountedForgeOp, CountingForge};
use temper_forge::{
    BranchRef, CreateIssue, CreatePullRequest, CreatePullRequestReview, CreateRepository, Forge,
    IssueQuery, IssueState, ItemListDetails, ItemNumber, MergeMethod, MergePullRequest,
    PullRequestQuery, PullRequestState, PullRequestUpdateState, RepositoryId, RequestReviewers,
    ReviewDecision, UpdateIssue, UpdatePullRequest, UserId,
};
use temper_forge_memory::MemoryForge;
use temper_runner::{candidate_query_plan, scan_role, ScanMode, WorkItem};
use temper_workflow::{ArtifactKindId, ArtifactSource, QueueId, RawWorkflowSpec, RoleId};

const REFERENCE_FIXTURE: &str =
    include_str!("../../temper-workflow/fixtures/reference-delivery.json");

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

fn closed_issue_queries_have_labels(queries: &[IssueQuery]) -> bool {
    queries
        .iter()
        .filter(|query| query.state == Some(IssueState::Closed))
        .all(|query| !query.labels.is_empty())
}

fn closed_pull_request_queries_have_labels(queries: &[PullRequestQuery]) -> bool {
    queries
        .iter()
        .filter(|query| {
            matches!(
                query.state,
                Some(PullRequestState::Closed | PullRequestState::Merged)
            )
        })
        .all(|query| !query.labels.is_empty())
}

fn has_issue_query(queries: &[IssueQuery], state: IssueState, labels: &[&str]) -> bool {
    queries.iter().any(|query| {
        query.state == Some(state)
            && query.labels
                == labels
                    .iter()
                    .map(|label| (*label).to_string())
                    .collect::<Vec<_>>()
    })
}

fn has_pull_request_query(
    queries: &[PullRequestQuery],
    state: PullRequestState,
    labels: &[&str],
) -> bool {
    queries.iter().any(|query| {
        query.state == Some(state)
            && query.labels
                == labels
                    .iter()
                    .map(|label| (*label).to_string())
                    .collect::<Vec<_>>()
    })
}

#[test]
fn candidate_planner_never_builds_closed_all_queries() {
    let workflow = workflow_from_json(PLANNER_FIXTURE);
    let compiled = workflow.compile();
    let role = RoleId::new("engineer");

    let normal = candidate_query_plan(&workflow, &compiled, Some(&role), ScanMode::Normal);
    assert!(closed_issue_queries_have_labels(&normal.issue_queries));
    assert!(closed_pull_request_queries_have_labels(
        &normal.pull_request_queries
    ));
    assert!(has_issue_query(
        &normal.issue_queries,
        IssueState::Open,
        &["ready", "urgent"]
    ));
    assert!(has_issue_query(
        &normal.issue_queries,
        IssueState::Closed,
        &["ready", "bug"]
    ));
    assert!(has_pull_request_query(
        &normal.pull_request_queries,
        PullRequestState::Open,
        &[]
    ));
    assert!(!normal.pull_request_queries.iter().any(|query| {
        matches!(
            query.state,
            Some(PullRequestState::Closed | PullRequestState::Merged)
        ) && query.labels.is_empty()
    }));

    let audit = candidate_query_plan(&workflow, &compiled, None, ScanMode::Audit);
    assert!(closed_issue_queries_have_labels(&audit.issue_queries));
    assert!(closed_pull_request_queries_have_labels(
        &audit.pull_request_queries
    ));
    assert!(has_issue_query(
        &audit.issue_queries,
        IssueState::Closed,
        &["code"]
    ));
    assert!(has_pull_request_query(
        &audit.pull_request_queries,
        PullRequestState::Merged,
        &["implementation"]
    ));
}

#[test]
fn overlapping_candidate_queries_deduplicate_artifacts() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let number = create_issue(&forge, &repo, &["code", "ready", "urgent", "bug"]);
    let workflow = workflow_from_json(PLANNER_FIXTURE);
    let compiled = workflow.compile();
    let counting = CountingForge::new(forge.clone());

    assert_eq!(
        block_on(scan_role(
            &counting,
            &repo,
            &workflow,
            &compiled,
            ts("2026-05-29T00:00:00Z"),
            &RoleId::new("engineer"),
        ))
        .expect("scan succeeds"),
        vec![WorkItem {
            queue: QueueId::new("branchy"),
            role: RoleId::new("engineer"),
            target: ArtifactSource::Issue { number },
            kind: ArtifactKindId::new("code"),
        }]
    );
    assert_eq!(counting.count(CountedForgeOp::ListIssues), 4);
}

#[test]
fn role_scan_does_not_request_closed_unlabelled_history() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let ready = create_issue(&forge, &repo, &["code", "ready"]);
    for _ in 0..25 {
        let issue = create_issue(&forge, &repo, &[]);
        close_issue(&forge, &repo, issue);
        let pull_request = create_pr(&forge, &repo, &[]);
        close_pr(&forge, &repo, pull_request);
    }
    let workflow = workflow_from_json(REFERENCE_FIXTURE);
    let compiled = workflow.compile();
    let counting = CountingForge::new(forge.clone());

    assert_eq!(
        block_on(scan_role(
            &counting,
            &repo,
            &workflow,
            &compiled,
            ts("2026-05-29T00:00:00Z"),
            &RoleId::new("engineer"),
        ))
        .expect("scan succeeds"),
        vec![WorkItem {
            queue: QueueId::new("code_ready"),
            role: RoleId::new("engineer"),
            target: ArtifactSource::Issue { number: ready },
            kind: ArtifactKindId::new("code"),
        }]
    );
    assert!(closed_issue_queries_have_labels(&counting.issue_queries()));
    assert!(closed_pull_request_queries_have_labels(
        &counting.pull_request_queries()
    ));
    assert_eq!(counting.count(CountedForgeOp::ListIssues), 2);
    assert!(!counting
        .issue_queries()
        .iter()
        .any(|query| query.state == Some(IssueState::Closed) && query.labels.is_empty()));
    assert!(!counting.pull_request_queries().iter().any(|query| {
        matches!(
            query.state,
            Some(PullRequestState::Closed | PullRequestState::Merged)
        ) && query.labels.is_empty()
    }));
}

#[test]
fn merged_pr_with_landed_queue_label_is_still_found() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let number = create_pr(&forge, &repo, &["implementation", "landed"]);
    merge_pr(&forge, &repo, number);
    let workflow = workflow_from_json(REFERENCE_FIXTURE);
    let compiled = workflow.compile();
    let counting = CountingForge::new(forge.clone());

    assert_eq!(
        block_on(scan_role(
            &counting,
            &repo,
            &workflow,
            &compiled,
            ts("2026-05-29T00:00:00Z"),
            &RoleId::new("architect"),
        ))
        .expect("scan succeeds"),
        vec![WorkItem {
            queue: QueueId::new("landed_inbox"),
            role: RoleId::new("architect"),
            target: ArtifactSource::PullRequest { number },
            kind: ArtifactKindId::new("implementation_pr"),
        }]
    );
    assert!(has_pull_request_query(
        &counting.pull_request_queries(),
        PullRequestState::Merged,
        &["landed"]
    ));
    assert!(counting.count(CountedForgeOp::ListPullRequests) > 0);
}

#[test]
fn open_all_fallback_queues_still_find_open_unlabelled_candidates() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let number = create_pr(&forge, &repo, &["implementation"]);
    submit_review(&forge, &repo, number, ReviewDecision::ChangesRequested);
    let workflow = workflow_from_json(REVIEW_ONLY_FIXTURE);
    let compiled = workflow.compile();
    let counting = CountingForge::new(forge.clone());

    assert_eq!(
        block_on(scan_role(
            &counting,
            &repo,
            &workflow,
            &compiled,
            ts("2026-05-29T00:00:00Z"),
            &RoleId::new("review_watcher"),
        ))
        .expect("scan succeeds"),
        vec![WorkItem {
            queue: QueueId::new("review_changes"),
            role: RoleId::new("review_watcher"),
            target: ArtifactSource::PullRequest { number },
            kind: ArtifactKindId::new("implementation_pr"),
        }]
    );
    assert!(counting.issue_queries().is_empty());
    assert_eq!(counting.count(CountedForgeOp::ListIssues), 0);
    assert_eq!(counting.count(CountedForgeOp::ListPullRequests), 1);
    assert_eq!(
        counting.pull_request_queries(),
        vec![PullRequestQuery {
            state: Some(PullRequestState::Open),
            labels: Vec::new(),
            details: ItemListDetails::summary(),
            ..PullRequestQuery::default()
        }]
    );
}
