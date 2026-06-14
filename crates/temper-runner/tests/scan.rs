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
use temper_workflow::{ArtifactKindId, ArtifactSource, QueueId, RawWorkflowSpec, RoleId};

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

#[test]
fn untriaged_issue_yields_architect_triage_work() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let number = create_issue(&forge, &repo, &["untriaged"]);

    assert_eq!(
        scan_repo(&forge, &repo),
        vec![WorkItem {
            queue: QueueId::new("design_triage"),
            role: RoleId::new("architect"),
            target: ArtifactSource::Issue { number },
            kind: ArtifactKindId::new("intake"),
        }]
    );
}

#[test]
fn ready_code_issue_yields_engineer_work() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let number = create_issue(&forge, &repo, &["code", "ready"]);

    assert_eq!(
        scan_repo(&forge, &repo),
        vec![WorkItem {
            queue: QueueId::new("code_ready"),
            role: RoleId::new("engineer"),
            target: ArtifactSource::Issue { number },
            kind: ArtifactKindId::new("code"),
        }]
    );
}

#[test]
fn failing_pr_ci_yields_engineer_work_but_passing_ci_does_not() {
    let workflow = workflow();
    let compiled = workflow.compile();
    let now = ts("2026-05-29T00:00:00Z");

    let failing_forge = MemoryForge::new();
    let failing_repo = new_repo(&failing_forge);
    let failing = create_pr(&failing_forge, &failing_repo, &["implementation"]);
    seed_ci(
        &failing_forge,
        &failing_repo,
        failing,
        CiJobConclusion::Failure,
    );

    assert_eq!(
        block_on(scan(
            &failing_forge,
            &failing_repo,
            &workflow,
            &compiled,
            now,
        ))
        .expect("scan succeeds"),
        vec![WorkItem {
            queue: QueueId::new("pr_ci_failed"),
            role: RoleId::new("engineer"),
            target: ArtifactSource::PullRequest { number: failing },
            kind: ArtifactKindId::new("implementation_pr"),
        }]
    );

    let passing_forge = MemoryForge::new();
    let passing_repo = new_repo(&passing_forge);
    let passing = create_pr(&passing_forge, &passing_repo, &["implementation"]);
    seed_ci(
        &passing_forge,
        &passing_repo,
        passing,
        CiJobConclusion::Success,
    );

    assert!(
        block_on(scan(
            &passing_forge,
            &passing_repo,
            &workflow,
            &compiled,
            now,
        ))
        .expect("scan succeeds")
        .is_empty()
    );
}

#[test]
fn empty_repo_yields_empty_worklist() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);

    assert!(scan_repo(&forge, &repo).is_empty());
}

#[test]
fn unclassified_artifacts_are_ignored() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    create_issue(&forge, &repo, &[]);

    assert!(scan_repo(&forge, &repo).is_empty());
}

#[test]
fn role_scan_returns_only_the_roles_subscribed_queues() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let untriaged = create_issue(&forge, &repo, &["untriaged"]);
    create_issue(&forge, &repo, &["code", "ready"]);
    let workflow = workflow();
    let compiled = workflow.compile();

    assert_eq!(
        block_on(scan_role(
            &forge,
            &repo,
            &workflow,
            &compiled,
            ts("2026-05-29T00:00:00Z"),
            &RoleId::new("architect"),
        ))
        .expect("scan succeeds"),
        vec![WorkItem {
            queue: QueueId::new("design_triage"),
            role: RoleId::new("architect"),
            target: ArtifactSource::Issue { number: untriaged },
            kind: ArtifactKindId::new("intake"),
        }]
    );
}

#[test]
fn role_scan_without_ci_gated_queue_does_not_list_ci_jobs() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    create_pr(&forge, &repo, &["implementation"]);
    let workflow = workflow();
    let compiled = workflow.compile();
    let counting = CountingForge::new(forge.clone());

    let items = block_on(scan_role(
        &counting,
        &repo,
        &workflow,
        &compiled,
        ts("2026-05-29T00:00:00Z"),
        &RoleId::new("architect"),
    ))
    .expect("scan succeeds");

    assert!(items.is_empty());
    assert_eq!(counting.count(CountedForgeOp::ListCiJobs), 0);
    assert!(
        counting
            .issue_queries()
            .iter()
            .all(|query| query.details == ItemListDetails::summary())
    );
    assert!(
        counting
            .pull_request_queries()
            .iter()
            .all(|query| query.details == ItemListDetails::summary())
    );
}

#[test]
fn ci_gated_automated_queue_fetches_ci_and_matches() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let number = create_pr(&forge, &repo, &["implementation", "landing"]);
    seed_ci(&forge, &repo, number, CiJobConclusion::Success);
    let workflow = workflow();
    let compiled = workflow.compile();
    let counting = CountingForge::new(forge.clone());

    assert_eq!(
        block_on(scan_automated_queues(
            &counting,
            &repo,
            &workflow,
            &compiled,
            ts("2026-05-29T00:00:00Z"),
        ))
        .expect("scan succeeds"),
        vec![AutomatedWorkItem {
            queue: QueueId::new("landing"),
            actor: RoleId::new("mechanical"),
            transition: temper_workflow::TransitionId::new("land_pr"),
            executor: None,
            outcomes: std::collections::BTreeMap::from([(
                temper_workflow::VerdictId::merge_conflict(),
                temper_workflow::TransitionId::new("route_merge_conflict"),
            )]),
            target: ArtifactSource::PullRequest { number },
            kind: ArtifactKindId::new("implementation_pr"),
        }]
    );
    assert_eq!(counting.count(CountedForgeOp::ListCiJobs), 1);
    assert_eq!(counting.count(CountedForgeOp::ListPullRequestReviews), 0);
}

#[test]
fn merged_landing_pr_with_passing_ci_is_not_an_automated_work_item() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let number = create_pr(&forge, &repo, &["implementation", "landing", "landed"]);
    seed_ci(&forge, &repo, number, CiJobConclusion::Success);
    merge_pr(&forge, &repo, number);
    let workflow = workflow();
    let compiled = workflow.compile();
    let counting = CountingForge::new(forge.clone());

    assert!(
        block_on(scan_automated_queues(
            &counting,
            &repo,
            &workflow,
            &compiled,
            ts("2026-05-29T00:00:00Z"),
        ))
        .expect("scan succeeds")
        .is_empty()
    );
    assert_eq!(counting.count(CountedForgeOp::ListCiJobs), 0);
}

#[test]
fn review_gated_queue_fetches_reviews_but_not_ci() {
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
    assert_eq!(counting.count(CountedForgeOp::ListPullRequestReviews), 1);
    assert_eq!(counting.count(CountedForgeOp::ListCiJobs), 0);
}

#[test]
fn dependency_gated_queue_fetches_dependency_state() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let dependency = create_issue(&forge, &repo, &["code"]);
    close_issue(&forge, &repo, dependency);
    let blocked = create_issue(&forge, &repo, &["code", "blocked"]);
    add_issue_dependency(&forge, &repo, blocked, dependency);
    let workflow = workflow_from_json(DEPENDENCY_QUEUE_FIXTURE);
    let compiled = workflow.compile();
    let counting = CountingForge::new(forge.clone());

    assert_eq!(
        block_on(scan_role(
            &counting,
            &repo,
            &workflow,
            &compiled,
            ts("2026-05-29T00:00:00Z"),
            &RoleId::new("dependency_watcher"),
        ))
        .expect("scan succeeds"),
        vec![WorkItem {
            queue: QueueId::new("dependencies_clear"),
            role: RoleId::new("dependency_watcher"),
            target: ArtifactSource::Issue { number: blocked },
            kind: ArtifactKindId::new("code"),
        }]
    );
    assert!(counting.count(CountedForgeOp::GetIssueByNumber) >= 2);
    assert!(
        counting
            .issue_queries()
            .iter()
            .all(|query| query.details == ItemListDetails::summary())
    );
    assert_eq!(counting.count(CountedForgeOp::ListCiJobs), 0);
    assert_eq!(counting.count(CountedForgeOp::ListPullRequestReviews), 0);
}

#[test]
fn unlabeled_issue_is_serviced_by_the_mechanical_intake_queue() {
    // A freshly filed human issue carries no labels at all. The default `intake`
    // kind admits it, and the empty-label `raw_intake` queue services it
    // mechanically, planning `mark_untriaged` to stamp the `untriaged` label.
    // This is intake flow step 1->2 (issue #35): unlabeled issue -> untriaged.
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let number = create_issue(&forge, &repo, &[]);
    let workflow = workflow_from_json(INTAKE_DEFAULT_FIXTURE);
    let compiled = workflow.compile();

    assert_eq!(
        block_on(scan_automated_queues(
            &forge,
            &repo,
            &workflow,
            &compiled,
            ts("2026-05-29T00:00:00Z"),
        ))
        .expect("scan succeeds"),
        vec![AutomatedWorkItem {
            queue: QueueId::new("raw_intake"),
            actor: RoleId::new("mechanical"),
            transition: temper_workflow::TransitionId::new("mark_untriaged"),
            executor: None,
            outcomes: std::collections::BTreeMap::new(),
            target: ArtifactSource::Issue { number },
            kind: ArtifactKindId::new("intake"),
        }]
    );
}

#[test]
fn labeled_issue_is_not_serviced_by_the_default_intake_queue() {
    // A labeled issue classifies as its specific kind (`code`), not the default
    // catch-all `intake`. The empty-label intake queue selects only intake
    // artifacts, so a `code` issue is left for its own queues: the default kind
    // does not change behavior for labeled issues.
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let _ = create_issue(&forge, &repo, &["code"]);
    let workflow = workflow_from_json(INTAKE_DEFAULT_FIXTURE);
    let compiled = workflow.compile();

    assert_eq!(
        block_on(scan_automated_queues(
            &forge,
            &repo,
            &workflow,
            &compiled,
            ts("2026-05-29T00:00:00Z"),
        ))
        .expect("scan succeeds"),
        Vec::new(),
    );
}

#[test]
fn reference_fixture_services_raw_intake_via_mechanical_queue() {
    // End-to-end guard for the demo's first hop: in the *canonical*
    // reference-delivery workflow, a freshly filed unlabeled human issue is
    // admitted by the default `intake` kind and serviced by the label-less
    // `raw_intake` automation queue, which plans the mechanical `mark_untriaged`
    // stamp. Without this queue the issue never gains `untriaged`, the
    // architect's `design_triage` queue never matches, and the whole pipeline
    // stalls at issue #1.
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let number = create_issue(&forge, &repo, &[]);
    let workflow = workflow();
    let compiled = workflow.compile();

    assert_eq!(
        block_on(scan_automated_queues(
            &forge,
            &repo,
            &workflow,
            &compiled,
            ts("2026-05-29T00:00:00Z"),
        ))
        .expect("scan succeeds"),
        vec![AutomatedWorkItem {
            queue: QueueId::new("raw_intake"),
            actor: RoleId::new("mechanical"),
            transition: temper_workflow::TransitionId::new("mark_untriaged"),
            executor: None,
            outcomes: std::collections::BTreeMap::new(),
            target: ArtifactSource::Issue { number },
            kind: ArtifactKindId::new("intake"),
        }]
    );
}
