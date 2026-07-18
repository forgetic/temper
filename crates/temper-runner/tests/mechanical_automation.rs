//! Mechanical worker coverage for declared queue automation.

mod support;

use chrono::{DateTime, Duration, Utc};
use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use support::{CountedForgeOp, CountingForge};
use temper_forge::{
    BranchRef, CandidateLabelSelection, CandidateLifecycle, ChangeKind, CreateIssue,
    CreatePullRequest, CreateRepository, Forge, HintArtifactKind, IssueCandidateQuery, IssueState,
    ItemListDetails, ItemNumber, PullRequestCandidateQuery, PullRequestState,
    PullRequestUpdateState, RepositoryId, UpdateIssue, UpdatePullRequest, UserId,
};
use temper_forge_memory::MemoryForge;
use temper_runner::{MechanicalWorker, Progress, Worker};
use temper_workflow::{InMemoryJournal, LeasePolicy, RawWorkflowSpec};

const AUTOMATED_LABEL_WORKFLOW: &str = r#"
{
  "name": "automated-labels",
  "roles": [
    { "id": "mechanical" }
  ],
  "labels": [
    { "id": "task" },
    { "id": "ready" },
    { "id": "done" },
    { "id": "approved" }
  ],
  "artifact_kinds": [
    {
      "id": "task",
      "target": "issue",
      "identifying_labels": ["task"]
    }
  ],
  "queues": [
    {
      "id": "ready_tasks",
      "artifact": "task",
      "labels": ["ready"],
      "automation": {
        "actor": "mechanical",
        "transition": "finish_task"
      }
    }
  ],
  "transitions": [
    {
      "id": "finish_task",
      "artifact": "task",
      "roles": ["mechanical"],
      "effects": [
        { "kind": "remove_label", "label": "ready" },
        { "kind": "add_label", "label": "done" }
      ]
    }
  ]
}
"#;

const GATED_AUTOMATED_LABEL_WORKFLOW: &str = r#"
{
  "name": "gated-automated-labels",
  "roles": [
    { "id": "mechanical" }
  ],
  "labels": [
    { "id": "task" },
    { "id": "ready" },
    { "id": "done" },
    { "id": "approved" }
  ],
  "artifact_kinds": [
    {
      "id": "task",
      "target": "issue",
      "identifying_labels": ["task"]
    }
  ],
  "queues": [
    {
      "id": "ready_tasks",
      "artifact": "task",
      "labels": ["ready"],
      "automation": {
        "actor": "mechanical",
        "transition": "finish_task"
      }
    }
  ],
  "transitions": [
    {
      "id": "finish_task",
      "artifact": "task",
      "roles": ["mechanical"],
      "requires_gates": ["approval_gate"],
      "effects": [
        { "kind": "remove_label", "label": "ready" },
        { "kind": "add_label", "label": "done" }
      ]
    }
  ],
  "gates": [
    {
      "id": "approval_gate",
      "condition": { "kind": "label_present", "label": "approved" }
    }
  ]
}
"#;

const AUTOMATED_PR_WORKFLOW: &str = r#"
{
  "name": "automated-pr-landing",
  "roles": [
    { "id": "mechanical" }
  ],
  "labels": [
    { "id": "implementation" },
    { "id": "landing" },
    { "id": "landed" },
    { "id": "approved" }
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
      "id": "landing",
      "artifact": "implementation_pr",
      "labels": ["landing"],
      "automation": {
        "actor": "mechanical",
        "transition": "land_pr"
      }
    }
  ],
  "transitions": [
    {
      "id": "land_pr",
      "artifact": "implementation_pr",
      "roles": ["mechanical"],
      "requires_gates": ["approval_gate"],
      "effects": [
        { "kind": "merge_pull_request" },
        { "kind": "remove_label", "label": "landing" },
        { "kind": "add_label", "label": "landed" }
      ]
    }
  ],
  "gates": [
    {
      "id": "approval_gate",
      "condition": { "kind": "label_present", "label": "approved" }
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

fn lease_policy() -> LeasePolicy {
    LeasePolicy::new(Duration::minutes(30))
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
            assignees: Vec::<UserId>::new(),
        },
    ))
    .expect("issue is created")
    .number
}

fn create_pull_request(forge: &MemoryForge, repo: &RepositoryId, labels: &[&str]) -> ItemNumber {
    block_on(forge.create_pull_request(
        repo,
        CreatePullRequest {
            title: "pr".into(),
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

fn close_pull_request(forge: &MemoryForge, repo: &RepositoryId, number: ItemNumber) {
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

fn add_issue_label(forge: &MemoryForge, repo: &RepositoryId, number: ItemNumber, label: &str) {
    let issue = block_on(forge.get_issue_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("issue exists");
    block_on(forge.update_issue(
        &issue.id,
        UpdateIssue {
            add_labels: vec![label.to_string()],
            ..UpdateIssue::default()
        },
    ))
    .expect("issue label added");
}

fn add_pull_request_label(
    forge: &MemoryForge,
    repo: &RepositoryId,
    number: ItemNumber,
    label: &str,
) {
    let pull_request = block_on(forge.get_pull_request_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("pull request exists");
    block_on(forge.update_pull_request(
        &pull_request.id,
        UpdatePullRequest {
            add_labels: vec![label.to_string()],
            ..UpdatePullRequest::default()
        },
    ))
    .expect("pull request label added");
}

fn issue_labels(forge: &MemoryForge, repo: &RepositoryId, number: ItemNumber) -> Vec<String> {
    let mut labels = block_on(forge.get_issue_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("issue exists")
        .labels;
    labels.sort();
    labels
}

fn pull_request_state(
    forge: &MemoryForge,
    repo: &RepositoryId,
    number: ItemNumber,
) -> PullRequestState {
    block_on(forge.get_pull_request_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("pull request exists")
        .state
}

fn is_bounded_issue_query(query: &IssueCandidateQuery) -> bool {
    query.details == ItemListDetails::summary()
        && (query.lifecycle == CandidateLifecycle::Open
            || matches!(query.labels, CandidateLabelSelection::AnyOf(_)))
}

fn is_bounded_pull_request_query(query: &PullRequestCandidateQuery) -> bool {
    query.details == ItemListDetails::summary()
        && (query.lifecycle == CandidateLifecycle::Open
            || matches!(query.labels, CandidateLabelSelection::AnyOf(_)))
}

#[test]
fn mechanical_tick_executes_automated_label_transition_without_agent() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let issue = create_issue(&forge, &repo, &["task", "ready"]);
    let workflow = workflow_from_json(AUTOMATED_LABEL_WORKFLOW);
    let journal = InMemoryJournal::new();
    let worker = MechanicalWorker::new(&workflow, &forge, &repo, &journal, lease_policy());

    assert_eq!(
        block_on(worker.tick(ts("2026-05-29T00:00:00Z"))).expect("tick succeeds"),
        Progress {
            changed: true,
            actions: 1,
        }
    );
    assert_eq!(
        issue_labels(&forge, &repo, issue),
        vec!["done".to_string(), "task".to_string()]
    );
    assert_eq!(
        block_on(worker.tick(ts("2026-05-29T00:00:01Z"))).expect("tick succeeds"),
        Progress::unchanged()
    );
}

#[test]
fn gate_not_satisfied_automated_item_is_unchanged_and_retried() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let issue = create_issue(&forge, &repo, &["task", "ready"]);
    let workflow = workflow_from_json(GATED_AUTOMATED_LABEL_WORKFLOW);
    let journal = InMemoryJournal::new();
    let worker = MechanicalWorker::new(&workflow, &forge, &repo, &journal, lease_policy());

    assert_eq!(
        block_on(worker.tick(ts("2026-05-29T00:00:00Z"))).expect("gate miss is nonfatal"),
        Progress::unchanged()
    );
    assert_eq!(
        issue_labels(&forge, &repo, issue),
        vec!["ready".to_string(), "task".to_string()]
    );

    add_issue_label(&forge, &repo, issue, "approved");
    assert_eq!(
        block_on(worker.tick(ts("2026-05-29T00:01:00Z"))).expect("retry succeeds"),
        Progress {
            changed: true,
            actions: 1,
        }
    );
    assert_eq!(
        issue_labels(&forge, &repo, issue),
        vec![
            "approved".to_string(),
            "done".to_string(),
            "task".to_string(),
        ]
    );
}

#[test]
fn automated_queue_scan_keeps_normal_tick_bounded() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    for _ in 0..10 {
        let issue = create_issue(&forge, &repo, &[]);
        close_issue(&forge, &repo, issue);
        let pull_request = create_pull_request(&forge, &repo, &[]);
        close_pull_request(&forge, &repo, pull_request);
    }
    create_issue(&forge, &repo, &["task", "ready"]);
    let counted = CountingForge::new(forge.clone());
    let workflow = workflow_from_json(AUTOMATED_LABEL_WORKFLOW);
    let journal = InMemoryJournal::new();
    let worker = MechanicalWorker::new(&workflow, &counted, &repo, &journal, lease_policy());

    assert_eq!(
        block_on(worker.tick(ts("2026-05-29T00:00:00Z"))).expect("tick succeeds"),
        Progress {
            changed: true,
            actions: 1,
        }
    );
    assert!(counted.issue_queries().is_empty());
    assert!(counted.pull_request_queries().is_empty());
    assert!(
        counted
            .issue_candidate_queries()
            .iter()
            .all(is_bounded_issue_query)
    );
    assert!(
        counted
            .pull_request_candidate_queries()
            .iter()
            .all(is_bounded_pull_request_query)
    );
}

#[test]
fn targeted_ci_wake_lands_pr_without_terminal_list_queries() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let ready = create_pull_request(&forge, &repo, &["implementation", "landing", "approved"]);
    let counted = CountingForge::new(forge.clone());
    let workflow = workflow_from_json(AUTOMATED_PR_WORKFLOW);
    let journal = InMemoryJournal::new();
    let worker = MechanicalWorker::new(&workflow, &counted, &repo, &journal, lease_policy());

    let progress = block_on(worker.tick_artifact(
        ts("2026-05-29T00:00:00Z"),
        ready,
        HintArtifactKind::PullRequest,
        ChangeKind::Ci,
    ))
    .expect("targeted CI tick succeeds");

    assert_eq!(
        progress,
        Progress {
            changed: true,
            actions: 1
        }
    );
    assert_eq!(counted.count(CountedForgeOp::MergePullRequest), 1);
    assert!(counted.count(CountedForgeOp::GetPullRequestByNumber) >= 1);
    assert_eq!(
        pull_request_state(&forge, &repo, ready),
        PullRequestState::Merged
    );
    assert!(counted.issue_candidate_queries().is_empty());
    assert!(counted.pull_request_candidate_queries().is_empty());
}

#[test]
fn automated_pr_merges_continue_after_gate_miss_and_retry_later() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let waiting = create_pull_request(&forge, &repo, &["implementation", "landing"]);
    let ready = create_pull_request(&forge, &repo, &["implementation", "landing", "approved"]);
    let counted = CountingForge::new(forge.clone());
    let workflow = workflow_from_json(AUTOMATED_PR_WORKFLOW);
    let journal = InMemoryJournal::new();
    let worker = MechanicalWorker::new(&workflow, &counted, &repo, &journal, lease_policy());

    assert_eq!(
        block_on(worker.tick(ts("2026-05-29T00:00:00Z"))).expect("first tick succeeds"),
        Progress {
            changed: true,
            actions: 1,
        }
    );
    assert_eq!(counted.count(CountedForgeOp::MergePullRequest), 1);
    assert!(counted.count(CountedForgeOp::GetPullRequestByNumber) >= 2);
    assert!(counted.pull_request_queries().is_empty());
    assert!(
        counted
            .pull_request_candidate_queries()
            .iter()
            .all(is_bounded_pull_request_query)
    );
    assert_eq!(
        pull_request_state(&forge, &repo, waiting),
        PullRequestState::Open
    );
    assert_eq!(
        pull_request_state(&forge, &repo, ready),
        PullRequestState::Merged
    );

    add_pull_request_label(&forge, &repo, waiting, "approved");
    assert_eq!(
        block_on(worker.tick(ts("2026-05-29T00:01:00Z"))).expect("retry succeeds"),
        Progress {
            changed: true,
            actions: 1,
        }
    );
    assert_eq!(counted.count(CountedForgeOp::MergePullRequest), 2);
    assert_eq!(
        pull_request_state(&forge, &repo, waiting),
        PullRequestState::Merged
    );
}
