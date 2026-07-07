//! Tests for runtime transition execution through a Forge backend (Phase 6).
//!
//! These drive the [`Executor`] against the deterministic in-memory backend:
//! create a repository and artifacts, then execute transitions and assert the
//! backend state, idempotency, and the typed failure classes.

mod support;

use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use temper_forge::{
    BranchRef, CiJob, CiJobConclusion, CiJobStatus, CreateComment, CreateIssue, CreatePullRequest,
    Forge, IssueState, ItemNumber, PullRequestState, RepositoryId, ReviewDecision, UpdateIssue,
    UserId,
};
use temper_forge_memory::{FaultOp, MemoryForge};
use temper_workflow::{
    ArtifactRef, ArtifactSource, ExecutionContext, ExecutionError, Executor, LabelId,
    PlanDiagnostic, RawWorkflowSpec, RoleId, TransitionId, ValidatedWorkflow, WorkflowEffect,
    WorkflowMetadata, parse_metadata_block, render_metadata_block,
};

const FIXTURE: &str = include_str!("../fixtures/ci-delivery.json");

const NON_LABEL_FIXTURE: &str = r#"
{
  "name": "non-label-execution",
  "roles": [{"id": "engineer"}, {"id": "owner"}],
  "labels": [
    {"id": "code"},
    {"id": "ready"},
    {"id": "in-progress"},
    {"id": "done"},
    {"id": "implementation"}
  ],
  "artifact_kinds": [
    {"id": "code", "target": "issue", "identifying_labels": ["code"]},
    {"id": "implementation_pr", "target": "pull_request", "identifying_labels": ["implementation"]}
  ],
  "transitions": [
    {"id": "claim_with_note", "artifact": "code", "roles": ["engineer"], "effects": [
      {"kind": "remove_label", "label": "ready"},
      {"kind": "add_label", "label": "in-progress"},
      {"kind": "set_assignee", "role": "engineer"},
      {"kind": "create_comment", "body": "Claimed for implementation."}
    ]},
    {"id": "finish_with_note", "artifact": "code", "roles": ["engineer"], "effects": [
      {"kind": "remove_label", "label": "in-progress"},
      {"kind": "add_label", "label": "done"},
      {"kind": "remove_assignee", "role": "engineer"},
      {"kind": "create_comment", "body": "Implementation finished."}
    ]},
    {"id": "open_pr", "artifact": "code", "roles": ["engineer"], "effects": [
      {"kind": "create_pull_request", "correlation_key": "pr-1"}
    ]},
    {"id": "merge_pr", "artifact": "implementation_pr", "roles": ["owner"], "effects": [
      {"kind": "merge_pull_request"}
    ]}
  ]
}
"#;

/// Owns one in-memory backend store for a test.
struct TestRoot {
    forge: MemoryForge,
}

impl TestRoot {
    fn new() -> Self {
        Self {
            forge: MemoryForge::new(),
        }
    }

    fn forge(&self) -> MemoryForge {
        self.forge.clone()
    }
}

struct NoopWake;

impl Wake for NoopWake {
    fn wake(self: Arc<Self>) {}
}

/// Drives a Forge future to completion; the in-memory backend never parks.
fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(NoopWake));
    let mut context = Context::from_waker(&waker);
    let mut future = Box::pin(future);
    match Future::poll(future.as_mut(), &mut context) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("in-memory forge futures should not park in tests"),
    }
}

fn workflow() -> ValidatedWorkflow {
    let spec: RawWorkflowSpec =
        serde_json::from_str(FIXTURE).expect("fixture is valid RawWorkflowSpec JSON");
    spec.validate().expect("CI delivery fixture validates")
}

fn non_label_workflow() -> ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(NON_LABEL_FIXTURE)
        .expect("non-label fixture is valid RawWorkflowSpec JSON");
    spec.validate().expect("non-label fixture validates")
}

fn new_repo(forge: &MemoryForge) -> RepositoryId {
    let repo = block_on(forge.create_repository(temper_forge::CreateRepository {
        owner: "acme".into(),
        name: "service".into(),
        default_branch: "main".into(),
        description: None,
    }))
    .expect("repository is created");
    repo.id
}

fn create_issue(forge: &MemoryForge, repo: &RepositoryId, labels: &[&str]) -> ItemNumber {
    create_issue_with_assignees(forge, repo, labels, Vec::new())
}

fn create_issue_with_assignees(
    forge: &MemoryForge,
    repo: &RepositoryId,
    labels: &[&str],
    assignees: Vec<UserId>,
) -> ItemNumber {
    block_on(forge.create_issue(
        repo,
        CreateIssue {
            title: "code work".into(),
            body: String::new(),
            labels: labels.iter().map(|l| (*l).to_string()).collect(),
            assignees,
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
            labels: labels.iter().map(|l| (*l).to_string()).collect(),
            assignees: Vec::new(),
        },
    ))
    .expect("pull request is created")
    .number
}

fn issue_labels(forge: &MemoryForge, repo: &RepositoryId, number: ItemNumber) -> Vec<String> {
    let mut labels = block_on(forge.get_issue_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("issue exists")
        .labels;
    labels.sort();
    labels
}

fn issue_assignees(forge: &MemoryForge, repo: &RepositoryId, number: ItemNumber) -> Vec<UserId> {
    block_on(forge.get_issue_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("issue exists")
        .assignees
}

fn issue_comments(forge: &MemoryForge, repo: &RepositoryId, number: ItemNumber) -> Vec<String> {
    let issue = block_on(forge.get_issue_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("issue exists");
    block_on(forge.list_issue_comments(&issue.id))
        .expect("comments list succeeds")
        .into_iter()
        .map(|comment| comment.body)
        .collect()
}

fn add_issue_comment(forge: &MemoryForge, repo: &RepositoryId, number: ItemNumber, body: &str) {
    let issue = block_on(forge.get_issue_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("issue exists");
    block_on(forge.add_issue_comment(&issue.id, CreateComment { body: body.into() }))
        .expect("comment is created");
}

#[test]
fn claim_transition_updates_labels_through_the_backend() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = workflow();
    let repo = new_repo(&forge);
    let number = create_issue(&forge, &repo, &["code", "ready"]);

    let executor = Executor::new(&workflow, &forge);
    let report = block_on(executor.execute(
        &repo,
        ArtifactSource::Issue { number },
        &TransitionId::new("claim_code"),
        &RoleId::new("engineer"),
    ))
    .expect("engineer can claim a ready code issue");

    assert_eq!(report.transition, TransitionId::new("claim_code"));
    assert_eq!(report.target, ArtifactSource::Issue { number });
    assert_eq!(
        report.applied,
        vec![
            WorkflowEffect::RemoveLabel("ready".into()),
            WorkflowEffect::AddLabel("in-progress".into()),
        ]
    );

    // The backend now reflects the claimed state.
    assert_eq!(
        issue_labels(&forge, &repo, number),
        vec!["code".to_string(), "in-progress".to_string()]
    );
}

#[test]
fn assignee_and_comment_effects_apply_once_under_retry() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = non_label_workflow();
    let repo = new_repo(&forge);
    let engineer = UserId::new("engineer-bot");
    let number =
        create_issue_with_assignees(&forge, &repo, &["code", "ready"], vec![engineer.clone()]);

    // SetAssignee is naturally idempotent when the user is already assigned.
    // Simulate a crash after the first comment write but before the state flip:
    // the retry must see the marker and not post the comment again.
    add_issue_comment(
        &forge,
        &repo,
        number,
        "Claimed for implementation.\n\n<!-- temper:comment-key=claim_with_note:0 -->",
    );

    let context = ExecutionContext::new().with_assignee(RoleId::new("engineer"), engineer.clone());
    let executor = workflow.executor_with_context(&forge, context);
    let report = block_on(executor.execute(
        &repo,
        ArtifactSource::Issue { number },
        &TransitionId::new("claim_with_note"),
        &RoleId::new("engineer"),
    ))
    .expect("engineer can claim with assignee and comment effects");

    assert_eq!(
        report.applied,
        vec![
            WorkflowEffect::RemoveLabel("ready".into()),
            WorkflowEffect::AddLabel("in-progress".into()),
            WorkflowEffect::SetAssignee {
                role: RoleId::new("engineer"),
            },
            WorkflowEffect::CreateComment {
                body: "Claimed for implementation.".into(),
            },
        ]
    );
    assert_eq!(
        issue_labels(&forge, &repo, number),
        vec!["code".to_string(), "in-progress".to_string()]
    );
    assert_eq!(
        issue_assignees(&forge, &repo, number),
        vec![engineer.clone()]
    );
    let comments = issue_comments(&forge, &repo, number);
    assert_eq!(comments.len(), 1);
    assert!(comments[0].contains("<!-- temper:comment-key=claim_with_note:0 -->"));

    let retry = block_on(executor.execute(
        &repo,
        ArtifactSource::Issue { number },
        &TransitionId::new("claim_with_note"),
        &RoleId::new("engineer"),
    ))
    .expect_err("a full retry sees fresh state and is refused");
    assert!(matches!(retry, ExecutionError::Precondition { .. }));
    assert_eq!(issue_comments(&forge, &repo, number).len(), 1);
    assert_eq!(
        issue_assignees(&forge, &repo, number),
        vec![engineer.clone()]
    );

    block_on(executor.execute(
        &repo,
        ArtifactSource::Issue { number },
        &TransitionId::new("finish_with_note"),
        &RoleId::new("engineer"),
    ))
    .expect("engineer can finish and clear the assignee");
    assert_eq!(
        issue_labels(&forge, &repo, number),
        vec!["code".to_string(), "done".to_string()]
    );
    assert!(issue_assignees(&forge, &repo, number).is_empty());
    let comments = issue_comments(&forge, &repo, number);
    assert_eq!(comments.len(), 2);
    assert!(comments[1].contains("<!-- temper:comment-key=finish_with_note:0 -->"));

    let retry = block_on(executor.execute(
        &repo,
        ArtifactSource::Issue { number },
        &TransitionId::new("finish_with_note"),
        &RoleId::new("engineer"),
    ))
    .expect_err("a finish retry is also refused by fresh preconditions");
    assert!(matches!(retry, ExecutionError::Precondition { .. }));
    assert_eq!(issue_comments(&forge, &repo, number).len(), 2);
}

#[test]
fn unresolved_assignee_refuses_before_comment_or_label_mutation() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = non_label_workflow();
    let repo = new_repo(&forge);
    let number = create_issue(&forge, &repo, &["code", "ready"]);

    let executor = Executor::new(&workflow, &forge);
    let error = block_on(executor.execute(
        &repo,
        ArtifactSource::Issue { number },
        &TransitionId::new("claim_with_note"),
        &RoleId::new("engineer"),
    ))
    .expect_err("assignee roles require runtime user bindings");

    assert_eq!(
        error,
        ExecutionError::UnresolvedAssignee {
            role: RoleId::new("engineer"),
        }
    );
    assert_eq!(
        issue_labels(&forge, &repo, number),
        vec!["code".to_string(), "ready".to_string()]
    );
    assert!(issue_assignees(&forge, &repo, number).is_empty());
    assert!(issue_comments(&forge, &repo, number).is_empty());
}

#[test]
fn merge_pull_request_effect_executes() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = non_label_workflow();
    let repo = new_repo(&forge);
    let executor = workflow.executor(&forge);
    let pr = create_pr(&forge, &repo, &["implementation"]);

    let report = block_on(executor.execute(
        &repo,
        ArtifactSource::PullRequest { number: pr },
        &TransitionId::new("merge_pr"),
        &RoleId::new("owner"),
    ))
    .expect("the owner merges the implementation pull request");
    assert_eq!(report.applied, vec![WorkflowEffect::MergePullRequest]);

    let merged = block_on(forge.get_pull_request_by_number(&repo, pr))
        .expect("lookup succeeds")
        .expect("pull request exists");
    assert_eq!(merged.state, PullRequestState::Merged);
    assert!(merged.merge.is_some(), "a merge record is recorded");
}

#[test]
fn approve_review_submits_a_native_review() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = workflow();
    let repo = new_repo(&forge);
    let number = create_pr(&forge, &repo, &["implementation", "needs-review"]);

    let executor = workflow.executor(&forge);
    let report = block_on(executor.execute(
        &repo,
        ArtifactSource::PullRequest { number },
        &TransitionId::new("approve_review"),
        &RoleId::new("reviewer"),
    ))
    .expect("reviewer can approve a pull request awaiting review");

    assert_eq!(
        report.applied,
        vec![
            WorkflowEffect::RemoveLabel("needs-review".into()),
            WorkflowEffect::SubmitReview {
                decision: ReviewDecision::Approved,
            },
        ]
    );

    let pull_request = block_on(forge.get_pull_request_by_number(&repo, number))
        .expect("lookup succeeds")
        .expect("pull request exists");
    assert_eq!(pull_request.labels, vec!["implementation".to_string()]);
    let reviews =
        block_on(forge.list_pull_request_reviews(&pull_request.id)).expect("reviews list succeeds");
    assert_eq!(reviews.len(), 1);
    assert_eq!(reviews[0].decision, ReviewDecision::Approved);
}

#[test]
fn stale_preconditions_prevent_mutation() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = workflow();
    let repo = new_repo(&forge);
    // Already in progress: re-claiming is stale (no `ready`) and contradicted.
    let number = create_issue(&forge, &repo, &["code", "in-progress"]);

    let executor = Executor::new(&workflow, &forge);
    let error = block_on(executor.execute(
        &repo,
        ArtifactSource::Issue { number },
        &TransitionId::new("claim_code"),
        &RoleId::new("engineer"),
    ))
    .expect_err("a re-claim must not execute");

    let ExecutionError::Precondition { diagnostics } = &error else {
        panic!("expected a precondition failure, got {error:?}");
    };
    assert!(diagnostics.contains(&PlanDiagnostic::StalePrecondition {
        transition: TransitionId::new("claim_code"),
        label: "ready".into(),
    }));
    assert!(
        diagnostics.contains(&PlanDiagnostic::ContradictedPrecondition {
            transition: TransitionId::new("claim_code"),
            label: "in-progress".into(),
        })
    );

    // The backend state is untouched: no partial mutation occurred.
    assert_eq!(
        issue_labels(&forge, &repo, number),
        vec!["code".to_string(), "in-progress".to_string()]
    );
}

#[test]
fn ensure_issue_is_idempotent_across_retries() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = workflow();
    let repo = new_repo(&forge);
    let executor = Executor::new(&workflow, &forge);

    let input = || CreateIssue {
        title: "Add login flow".into(),
        body: "Implements login.".into(),
        labels: vec!["code".into(), "ready".into()],
        assignees: Vec::<UserId>::new(),
    };

    let first = block_on(executor.ensure_issue(&repo, "code-issue-42", input()))
        .expect("first ensure creates the issue");
    assert!(first.was_created());

    // The created issue carries the correlation key so retries can find it.
    let created = first.artifact();
    let metadata = parse_metadata_block(&created.body)
        .expect("body metadata parses")
        .expect("body has a metadata block");
    assert_eq!(metadata.correlation_key.as_deref(), Some("code-issue-42"));

    let second = block_on(executor.ensure_issue(&repo, "code-issue-42", input()))
        .expect("second ensure finds the existing issue");
    assert!(!second.was_created());
    assert_eq!(second.artifact().number, created.number);
    assert_eq!(second.artifact().id, created.id);

    // Exactly one issue exists; the retry did not duplicate it.
    let issues = block_on(forge.list_issues(&repo, temper_forge::IssueQuery::default()))
        .expect("issues list");
    assert_eq!(issues.len(), 1);
}

#[test]
fn execution_diagnostics_distinguish_failure_classes() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = workflow();
    let repo = new_repo(&forge);
    let executor = Executor::new(&workflow, &forge);

    // Validation failure: a reviewer is not authorized to claim code.
    let ready = create_issue(&forge, &repo, &["code", "ready"]);
    let validation = block_on(executor.execute(
        &repo,
        ArtifactSource::Issue { number: ready },
        &TransitionId::new("claim_code"),
        &RoleId::new("reviewer"),
    ))
    .expect_err("an unauthorized role fails validation");
    let ExecutionError::Validation { diagnostics } = &validation else {
        panic!("expected a validation failure, got {validation:?}");
    };
    assert!(diagnostics.contains(&PlanDiagnostic::Unauthorized {
        transition: TransitionId::new("claim_code"),
        role: RoleId::new("reviewer"),
    }));

    // Precondition failure: a merge cannot proceed until its gates are met.
    let pr = create_pr(&forge, &repo, &["implementation"]);
    let precondition = block_on(executor.execute(
        &repo,
        ArtifactSource::PullRequest { number: pr },
        &TransitionId::new("approve_merge"),
        &RoleId::new("owner"),
    ))
    .expect_err("an ungated merge fails preconditions");
    assert!(matches!(precondition, ExecutionError::Precondition { .. }));

    // Backend failure: arm a one-shot fault so the executor's load fails.
    forge.fail_next(
        FaultOp::GetIssueByNumber,
        "simulated unreachable backend store",
    );
    let backend = block_on(executor.execute(
        &repo,
        ArtifactSource::Issue { number: ready },
        &TransitionId::new("claim_code"),
        &RoleId::new("engineer"),
    ))
    .expect_err("a malformed backend store surfaces a backend error");
    assert!(matches!(backend, ExecutionError::Backend { .. }));
}

#[test]
fn missing_target_is_reported_distinctly() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = workflow();
    let repo = new_repo(&forge);
    let executor = Executor::new(&workflow, &forge);

    let error = block_on(executor.execute(
        &repo,
        ArtifactSource::Issue {
            number: ItemNumber::new(999),
        },
        &TransitionId::new("claim_code"),
        &RoleId::new("engineer"),
    ))
    .expect_err("a missing artifact cannot be executed");
    assert!(matches!(
        error,
        ExecutionError::TargetMissing {
            target: ArtifactSource::Issue { .. }
        }
    ));
}

// ── close_parent_issues execution tests ──

const CLOSE_PARENTS_FIXTURE: &str = r#"
{
  "name": "close-parents-execution",
  "roles": [{"id": "engineer"}],
  "labels": [
    {"id": "code"},
    {"id": "in-progress"},
    {"id": "implementation"}
  ],
  "artifact_kinds": [
    {"id": "code", "target": "issue", "identifying_labels": ["code"]},
    {"id": "implementation_pr", "target": "pull_request", "identifying_labels": ["implementation"]}
  ],
  "transitions": [
    {
      "id": "land_pr",
      "artifact": "implementation_pr",
      "roles": ["engineer"],
      "effects": [
        {"kind": "merge_pull_request"},
        {"kind": "close_parent_issues"}
      ]
    },
    {
      "id": "close_parent_only",
      "artifact": "implementation_pr",
      "roles": ["engineer"],
      "effects": [
        {"kind": "close_parent_issues"}
      ]
    }
  ]
}
"#;

fn close_parents_workflow() -> ValidatedWorkflow {
    let spec: RawWorkflowSpec =
        serde_json::from_str(CLOSE_PARENTS_FIXTURE).expect("fixture valid JSON");
    spec.validate().expect("close-parents fixture validates")
}

fn ts() -> chrono::DateTime<chrono::Utc> {
    "2026-05-29T00:00:00Z".parse().expect("valid timestamp")
}

fn create_pr_with_body(
    forge: &MemoryForge,
    repo: &RepositoryId,
    labels: &[&str],
    body: &str,
) -> ItemNumber {
    block_on(forge.create_pull_request(
        repo,
        CreatePullRequest {
            title: "implementation".into(),
            body: body.to_string(),
            source: BranchRef {
                repository_id: repo.clone(),
                branch: "feature".into(),
            },
            target: BranchRef {
                repository_id: repo.clone(),
                branch: "main".into(),
            },
            labels: labels.iter().map(|l| (*l).to_string()).collect(),
            assignees: Vec::new(),
        },
    ))
    .expect("pull request is created")
    .number
}

#[test]
fn close_parent_issues_closes_open_same_repo_parents() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = close_parents_workflow();
    let repo = new_repo(&forge);

    let plan_parent = create_issue(&forge, &repo, &["code", "in-progress"]);
    let feature_parent = create_issue(&forge, &repo, &["code", "in-progress"]);

    let metadata = temper_workflow::WorkflowMetadata {
        parents: vec![
            ArtifactRef::same_repo(plan_parent),
            ArtifactRef::same_repo(feature_parent),
        ],
        ..Default::default()
    };
    let block = temper_workflow::render_metadata_block(&metadata);
    let pr_number = create_pr_with_body(&forge, &repo, &["implementation"], &block);

    let executor = workflow.executor(&forge);
    block_on(executor.execute(
        &repo,
        ArtifactSource::PullRequest { number: pr_number },
        &TransitionId::new("close_parent_only"),
        &RoleId::new("engineer"),
    ))
    .expect("close_parent_issues executes on PR with parent metadata");

    for parent_number in [plan_parent, feature_parent] {
        let parent = block_on(forge.get_issue_by_number(&repo, parent_number))
            .expect("lookup succeeds")
            .expect("parent issue exists");
        assert_eq!(parent.state, IssueState::Closed);
        assert!(!parent.labels.contains(&"in-progress".to_string()));
    }
}

#[test]
fn close_parent_issues_is_idempotent_on_already_closed_parent() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = close_parents_workflow();
    let repo = new_repo(&forge);

    // Create a parent issue already closed without in-progress.
    let parent_number = create_issue(&forge, &repo, &["code"]);
    // Pre-close it.
    let parent_issue = block_on(forge.get_issue_by_number(&repo, parent_number))
        .expect("lookup succeeds")
        .expect("parent exists");
    block_on(forge.update_issue(
        &parent_issue.id,
        UpdateIssue {
            state: Some(IssueState::Closed),
            ..UpdateIssue::default()
        },
    ))
    .expect("close succeeds");

    let metadata = temper_workflow::WorkflowMetadata {
        parents: vec![ArtifactRef::same_repo(parent_number)],
        ..Default::default()
    };
    let block = temper_workflow::render_metadata_block(&metadata);
    let pr_number = create_pr_with_body(&forge, &repo, &["implementation"], &block);

    let executor = workflow.executor(&forge);
    block_on(executor.execute(
        &repo,
        ArtifactSource::PullRequest { number: pr_number },
        &TransitionId::new("close_parent_only"),
        &RoleId::new("engineer"),
    ))
    .expect("close_parent_issues is idempotent on already-closed parent");

    // Parent remains closed.
    let parent = block_on(forge.get_issue_by_number(&repo, parent_number))
        .expect("lookup succeeds")
        .expect("parent issue exists");
    assert_eq!(parent.state, IssueState::Closed);
}

#[test]
fn close_parent_issues_handles_missing_metadata() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = close_parents_workflow();
    let repo = new_repo(&forge);

    // PR with no metadata block at all.
    let pr_number = create_pr(&forge, &repo, &["implementation"]);

    let executor = workflow.executor(&forge);
    block_on(executor.execute(
        &repo,
        ArtifactSource::PullRequest { number: pr_number },
        &TransitionId::new("close_parent_only"),
        &RoleId::new("engineer"),
    ))
    .expect("close_parent_issues with no metadata does not crash");
}

#[test]
fn close_parent_issues_handles_non_existent_parent() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = close_parents_workflow();
    let repo = new_repo(&forge);

    // Metadata references a non-existent parent.
    let metadata = temper_workflow::WorkflowMetadata {
        parents: vec![ArtifactRef::same_repo(ItemNumber::new(999))],
        ..Default::default()
    };
    let block = temper_workflow::render_metadata_block(&metadata);
    let pr_number = create_pr_with_body(&forge, &repo, &["implementation"], &block);

    let executor = workflow.executor(&forge);
    block_on(executor.execute(
        &repo,
        ArtifactSource::PullRequest { number: pr_number },
        &TransitionId::new("close_parent_only"),
        &RoleId::new("engineer"),
    ))
    .expect("close_parent_issues with non-existent parent does not crash");
}

/// The basic-delivery fixture, used to verify the full `land_pr` transition.
const BASIC_DELIVERY_FIXTURE: &str = include_str!("../fixtures/basic-delivery.json");

fn basic_delivery_workflow() -> ValidatedWorkflow {
    let spec: RawWorkflowSpec =
        serde_json::from_str(BASIC_DELIVERY_FIXTURE).expect("basic-delivery fixture valid JSON");
    spec.validate().expect("basic-delivery fixture validates")
}

#[test]
fn basic_delivery_land_pr_closes_parent_code_issue() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = basic_delivery_workflow();
    let repo = new_repo(&forge);

    // Create a parent code issue with in-progress label.
    let parent_number = create_issue(&forge, &repo, &["code", "in-progress"]);

    // Create an implementation PR with metadata pointing to the parent.
    let metadata = WorkflowMetadata {
        parents: vec![ArtifactRef::same_repo(parent_number)],
        ..Default::default()
    };
    let block = render_metadata_block(&metadata);
    let pr_number = create_pr_with_body(&forge, &repo, &["implementation", "landing"], &block);

    // Seed CI jobs so the ci_gate is satisfied.
    let pr = block_on(forge.get_pull_request_by_number(&repo, pr_number))
        .expect("lookup succeeds")
        .expect("pull request exists");
    let head_sha = pr.head_sha.clone().unwrap_or_else(|| "sha".to_string());
    forge.seed_ci_jobs(
        &repo,
        vec![CiJob {
            id: "ci-job-1".into(),
            repo_id: repo.clone(),
            pull_request_id: Some(pr.id.clone()),
            commit_sha: head_sha.clone(),
            name: "ci".into(),
            status: CiJobStatus::Completed,
            conclusion: Some(CiJobConclusion::Success),
            url: None,
            created_at: ts(),
            started_at: None,
            completed_at: None,
            updated_at: ts(),
        }],
    );

    let context = ExecutionContext::new();
    let crash = support::crash::CrashForge::new(forge.clone(), Vec::new());
    let executor = workflow.executor_with_context(&crash, context);

    // Pre-plan: check that land_pr plans with merge, landing-label cleanup, and close parents.
    let plan = block_on(executor.plan(
        &repo,
        ArtifactSource::PullRequest { number: pr_number },
        &TransitionId::new("land_pr"),
        &RoleId::new("mechanical"),
    ))
    .expect("land_pr plans with CI passed and no deps");
    assert_eq!(
        plan.effects,
        vec![
            WorkflowEffect::MergePullRequest,
            WorkflowEffect::RemoveLabel(LabelId::new("landing")),
            WorkflowEffect::CloseParentIssues
        ]
    );

    // Execute land_pr — merge + landing-label cleanup + close parent issues.
    let report = block_on(executor.execute(
        &repo,
        ArtifactSource::PullRequest { number: pr_number },
        &TransitionId::new("land_pr"),
        &RoleId::new("mechanical"),
    ))
    .expect("land_pr executes on a green PR");
    assert_eq!(
        report.applied,
        vec![
            WorkflowEffect::MergePullRequest,
            WorkflowEffect::RemoveLabel(LabelId::new("landing")),
            WorkflowEffect::CloseParentIssues
        ]
    );

    assert_eq!(
        crash
            .merge_inputs()
            .first()
            .map(|input| input.delete_source_branch),
        Some(true),
        "direct mechanical landings request PR head branch cleanup"
    );

    // The PR should now be merged.
    let merged_pr = block_on(forge.get_pull_request_by_number(&repo, pr_number))
        .expect("lookup succeeds")
        .expect("pull request exists");
    assert_eq!(merged_pr.state, PullRequestState::Merged);
    assert!(!merged_pr.labels.contains(&"landing".to_string()));

    // Parent code issue should now be closed and in-progress removed.
    let parent = block_on(forge.get_issue_by_number(&repo, parent_number))
        .expect("lookup succeeds")
        .expect("parent issue exists");
    assert_eq!(parent.state, IssueState::Closed);
    assert!(!parent.labels.contains(&"in-progress".to_string()));
}
