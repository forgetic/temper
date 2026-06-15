//! Tests for runtime transition execution through a Forge backend (Phase 6).
//!
//! These drive the [`Executor`] against the deterministic in-memory backend:
//! create a repository and artifacts, then execute transitions and assert the
//! backend state, idempotency, and the typed failure classes.

use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use temper_forge_model::{
    BranchRef, CreateComment, CreateIssue, CreatePullRequest, Forge, ItemNumber, PullRequestState,
    RepositoryId, ReviewDecision, UserId,
};
use temper_forge_memory::{FaultOp, MemoryForge};
use temper_workflow::{
    ArtifactSource, ExecutionContext, ExecutionError, Executor, PlanDiagnostic, RawWorkflowSpec,
    RoleId, TransitionId, ValidatedWorkflow, WorkflowEffect, parse_metadata_block,
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
    let repo = block_on(forge.create_repository(temper_forge_model::CreateRepository {
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
    let issues = block_on(forge.list_issues(&repo, temper_forge_model::IssueQuery::default()))
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
