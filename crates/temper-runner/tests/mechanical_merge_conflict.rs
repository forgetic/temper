//! Mechanical automation fallback coverage for merge conflicts.

mod support;

use chrono::{DateTime, Duration, Utc};
use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use support::{CountedForgeOp, CountingForge};
use temper_forge::{
    BranchRef, CiJob, CiJobConclusion, CiJobId, CiJobStatus, CreatePullRequest,
    CreatePullRequestReview, CreateRepository, Forge, ItemNumber, PullRequestState, RepositoryId,
    RequestReviewers, ReviewDecision, UserId,
};
use temper_forge_memory::MemoryForge;
use temper_runner::{MechanicalWorker, Progress, Worker};
use temper_workflow::{
    ArtifactSource, InMemoryJournal, LeasePolicy, RawWorkflowSpec, RoleId, TransitionId,
};

const AUTOMATED_CONFLICT_WORKFLOW: &str = r#"
{
  "name": "automated-conflict-routing",
  "roles": [
    { "id": "mechanical" }
  ],
  "labels": [
    { "id": "implementation" },
    { "id": "landing" },
    { "id": "landed" },
    { "id": "approved" },
    { "id": "merge-conflict" }
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
      "excluded_labels": ["merge-conflict"],
      "condition": { "kind": "ci_passed" },
      "automation": {
        "actor": "mechanical",
        "transition": "land_pr",
        "on_merge_conflict": "route_merge_conflict"
      }
    }
  ],
  "transitions": [
    {
      "id": "land_pr",
      "artifact": "implementation_pr",
      "roles": ["mechanical"],
      "requires_gates": ["approval_gate", "ci_gate"],
      "effects": [
        { "kind": "merge_pull_request" },
        { "kind": "remove_label", "label": "landing" },
        { "kind": "add_label", "label": "landed" }
      ]
    },
    {
      "id": "route_merge_conflict",
      "artifact": "implementation_pr",
      "roles": ["mechanical"],
      "effects": [
        { "kind": "add_label", "label": "merge-conflict" }
      ]
    }
  ],
  "gates": [
    {
      "id": "approval_gate",
      "condition": { "kind": "label_present", "label": "approved" }
    },
    {
      "id": "ci_gate",
      "condition": { "kind": "ci_passed" }
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
    let spec: RawWorkflowSpec =
        serde_json::from_str(AUTOMATED_CONFLICT_WORKFLOW).expect("workflow parses");
    spec.validate().expect("workflow validates")
}

fn workflow_without_conflict_route() -> temper_workflow::ValidatedWorkflow {
    let mut raw: serde_json::Value =
        serde_json::from_str(AUTOMATED_CONFLICT_WORKFLOW).expect("workflow parses");
    raw["queues"][0]["automation"]
        .as_object_mut()
        .expect("automation is an object")
        .remove("on_merge_conflict");
    let spec: RawWorkflowSpec = serde_json::from_value(raw).expect("workflow value parses");
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

fn pull_request_id(
    forge: &MemoryForge,
    repo: &RepositoryId,
    number: ItemNumber,
) -> temper_forge::PullRequestId {
    block_on(forge.get_pull_request_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("pull request exists")
        .id
}

fn seed_successful_ci(forge: &MemoryForge, repo: &RepositoryId, numbers: &[ItemNumber]) {
    let jobs = numbers
        .iter()
        .map(|number| {
            let pull_request = block_on(forge.get_pull_request_by_number(repo, *number))
                .expect("lookup succeeds")
                .expect("pull request exists");
            CiJob {
                id: CiJobId::new(format!("ci-{}-{}", repo.as_str(), number.get())),
                repo_id: repo.clone(),
                pull_request_id: Some(pull_request.id),
                commit_sha: format!("pr-{}-head", number.get()),
                name: "ci".into(),
                status: CiJobStatus::Completed,
                conclusion: Some(CiJobConclusion::Success),
                url: None,
                created_at: ts("2026-05-29T00:00:00Z"),
                started_at: Some(ts("2026-05-29T00:00:30Z")),
                completed_at: Some(ts("2026-05-29T00:01:00Z")),
                updated_at: ts("2026-05-29T00:01:00Z"),
            }
        })
        .collect();
    forge.seed_ci_jobs(repo, jobs);
}

fn seed_ci_for_head(
    forge: &MemoryForge,
    repo: &RepositoryId,
    number: ItemNumber,
    commit_sha: String,
    index: usize,
    status: CiJobStatus,
    conclusion: Option<CiJobConclusion>,
) {
    let pull_request = block_on(forge.get_pull_request_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("pull request exists");
    let mut jobs = block_on(forge.list_ci_jobs(repo, Default::default())).expect("CI jobs list");
    let offset = Duration::seconds(index as i64);
    jobs.push(CiJob {
        id: CiJobId::new(format!("ci-{}-{}-{index}", repo.as_str(), number.get())),
        repo_id: repo.clone(),
        pull_request_id: Some(pull_request.id),
        commit_sha,
        name: "ci".into(),
        status,
        conclusion,
        url: None,
        created_at: ts("2026-05-29T00:00:00Z") + offset,
        started_at: (status != CiJobStatus::Queued).then_some(ts("2026-05-29T00:00:30Z") + offset),
        completed_at: (status == CiJobStatus::Completed)
            .then_some(ts("2026-05-29T00:01:00Z") + offset),
        updated_at: ts("2026-05-29T00:01:00Z") + offset,
    });
    forge.seed_ci_jobs(repo, jobs);
}

fn seed_successful_ci_for_head(
    forge: &MemoryForge,
    repo: &RepositoryId,
    number: ItemNumber,
    commit_sha: String,
    index: usize,
) {
    seed_ci_for_head(
        forge,
        repo,
        number,
        commit_sha,
        index,
        CiJobStatus::Completed,
        Some(CiJobConclusion::Success),
    );
}

fn approve_as_requested_reviewer(forge: &MemoryForge, repo: &RepositoryId, number: ItemNumber) {
    let pull_request = block_on(forge.get_pull_request_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("pull request exists");
    block_on(forge.request_pull_request_reviewers(
        &pull_request.id,
        RequestReviewers {
            reviewers: vec![UserId::new("reviewer")],
        },
    ))
    .expect("reviewer requested");
    let reviewer_forge = forge.as_user(temper_testing::actor_user("reviewer"));
    block_on(reviewer_forge.submit_pull_request_review(
        &pull_request.id,
        CreatePullRequestReview {
            decision: ReviewDecision::Approved,
            body: None,
        },
    ))
    .expect("review submitted");
}

fn pull_request_labels(
    forge: &MemoryForge,
    repo: &RepositoryId,
    number: ItemNumber,
) -> Vec<String> {
    let mut labels = block_on(forge.get_pull_request_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("pull request exists")
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

fn projected_head<F: Forge>(
    counted: &CountingForge<F>,
    repo: &RepositoryId,
    number: ItemNumber,
) -> String {
    let pull_request = block_on(counted.get_pull_request_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("pull request exists");
    counted
        .projected_head(&pull_request)
        .expect("synthetic head is projected")
}

#[test]
fn mechanical_conflict_fallback_preserves_landing_and_stops_retry_loop() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let conflicted = create_pull_request(&forge, &repo, &["implementation", "landing", "approved"]);
    seed_successful_ci(&forge, &repo, &[conflicted]);
    let counted = CountingForge::new(forge.clone());
    counted.reject_merge_for(
        pull_request_id(&forge, &repo, conflicted),
        "simulated content conflict; provider response body intentionally omitted",
    );
    let workflow = workflow();
    let journal = InMemoryJournal::new();
    let worker = MechanicalWorker::new(&workflow, &counted, &repo, &journal, lease_policy());

    assert_eq!(
        block_on(worker.tick(ts("2026-05-29T00:00:00Z"))).expect("conflict route succeeds"),
        Progress {
            changed: true,
            actions: 1,
        }
    );
    assert_eq!(counted.count(CountedForgeOp::MergePullRequest), 1);
    assert_eq!(
        pull_request_labels(&forge, &repo, conflicted),
        vec![
            "approved".to_string(),
            "implementation".to_string(),
            "landing".to_string(),
            "merge-conflict".to_string(),
        ]
    );
    assert_eq!(
        pull_request_state(&forge, &repo, conflicted),
        PullRequestState::Open
    );

    assert_eq!(
        block_on(worker.tick(ts("2026-05-29T00:01:00Z"))).expect("later tick is quiet"),
        Progress::unchanged()
    );
    assert_eq!(
        counted.count(CountedForgeOp::MergePullRequest),
        1,
        "merge-conflict excludes the PR from landing automation without clearing landing"
    );
}

#[test]
fn unrelated_landing_pr_continues_after_another_pr_routes_to_conflict() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let conflicted = create_pull_request(&forge, &repo, &["implementation", "landing", "approved"]);
    let ready = create_pull_request(&forge, &repo, &["implementation", "landing", "approved"]);
    seed_successful_ci(&forge, &repo, &[conflicted, ready]);
    let counted = CountingForge::new(forge.clone());
    counted.reject_merge_for(
        pull_request_id(&forge, &repo, conflicted),
        "simulated content conflict",
    );
    let workflow = workflow();
    let journal = InMemoryJournal::new();
    let worker = MechanicalWorker::new(&workflow, &counted, &repo, &journal, lease_policy());

    assert_eq!(
        block_on(worker.tick(ts("2026-05-29T00:00:00Z"))).expect("tick succeeds"),
        Progress {
            changed: true,
            actions: 2,
        }
    );
    assert_eq!(counted.count(CountedForgeOp::MergePullRequest), 2);
    assert_eq!(
        pull_request_labels(&forge, &repo, conflicted),
        vec![
            "approved".to_string(),
            "implementation".to_string(),
            "landing".to_string(),
            "merge-conflict".to_string(),
        ]
    );
    assert_eq!(
        pull_request_state(&forge, &repo, ready),
        PullRequestState::Merged
    );
    assert!(pull_request_labels(&forge, &repo, ready).contains(&"landed".to_string()));
}

#[test]
fn unhandled_merge_conflict_does_not_starve_later_landing_candidates() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let conflicted = create_pull_request(&forge, &repo, &["implementation", "landing", "approved"]);
    let ready = create_pull_request(&forge, &repo, &["implementation", "landing", "approved"]);
    seed_successful_ci(&forge, &repo, &[conflicted, ready]);
    let counted = CountingForge::new(forge.clone());
    counted.reject_merge_for(
        pull_request_id(&forge, &repo, conflicted),
        "simulated content conflict without workflow fallback",
    );
    let workflow = workflow_without_conflict_route();
    let journal = InMemoryJournal::new();
    let worker = MechanicalWorker::new(&workflow, &counted, &repo, &journal, lease_policy());

    assert_eq!(
        block_on(worker.tick(ts("2026-05-29T00:00:00Z"))).expect("tick continues"),
        Progress {
            changed: true,
            actions: 1,
        }
    );
    assert_eq!(counted.count(CountedForgeOp::MergePullRequest), 2);
    assert_eq!(
        pull_request_state(&forge, &repo, conflicted),
        PullRequestState::Open
    );
    assert_eq!(
        pull_request_state(&forge, &repo, ready),
        PullRequestState::Merged
    );
}

#[test]
fn reference_conflict_resolution_requires_fresh_ci_but_no_second_review() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let number = create_pull_request(&forge, &repo, &["implementation", "landing"]);
    approve_as_requested_reviewer(&forge, &repo, number);
    let counted = CountingForge::new(forge.clone());
    counted.enable_synthetic_pull_request_heads();
    counted.advance_heads_on_conflict_resolution();
    let old_head = projected_head(&counted, &repo, number);
    seed_successful_ci_for_head(&forge, &repo, number, old_head, 1);
    let pr_id = pull_request_id(&forge, &repo, number);
    counted.reject_merge_for(pr_id.clone(), "simulated content conflict");
    let workflow = temper_testing::workflow();
    let journal = InMemoryJournal::new();
    let worker = MechanicalWorker::new(&workflow, &counted, &repo, &journal, lease_policy());

    assert_eq!(
        block_on(worker.tick(ts("2026-05-29T00:00:00Z"))).expect("conflict routes"),
        Progress {
            changed: true,
            actions: 1,
        }
    );
    assert_eq!(counted.count(CountedForgeOp::MergePullRequest), 1);
    assert_eq!(
        pull_request_labels(&forge, &repo, number),
        vec![
            "implementation".to_string(),
            "landing".to_string(),
            "merge-conflict".to_string(),
        ]
    );

    counted.allow_merge_for(&pr_id);
    block_on(workflow.executor(&counted).execute(
        &repo,
        ArtifactSource::PullRequest { number },
        &TransitionId::new("resolve_merge_conflict"),
        &RoleId::new("engineer"),
    ))
    .expect("engineer requeues conflict resolution");
    let new_head = projected_head(&counted, &repo, number);
    assert_ne!(new_head, format!("pr-{}-head", number.get()));
    assert_eq!(
        pull_request_labels(&forge, &repo, number),
        vec!["implementation".to_string(), "landing".to_string()]
    );

    assert_eq!(
        block_on(worker.tick(ts("2026-05-29T00:01:00Z"))).expect("old CI is not enough"),
        Progress::unchanged()
    );
    assert_eq!(
        pull_request_state(&forge, &repo, number),
        PullRequestState::Open
    );
    assert_eq!(
        counted.count(CountedForgeOp::MergePullRequest),
        1,
        "old green CI must not satisfy the repaired head"
    );

    seed_ci_for_head(
        &forge,
        &repo,
        number,
        new_head.clone(),
        2,
        CiJobStatus::Queued,
        None,
    );
    assert_eq!(
        block_on(worker.tick(ts("2026-05-29T00:02:00Z"))).expect("queued CI is not enough"),
        Progress::unchanged()
    );
    assert_eq!(
        pull_request_state(&forge, &repo, number),
        PullRequestState::Open
    );
    assert_eq!(
        counted.count(CountedForgeOp::MergePullRequest),
        1,
        "queued repaired-head CI must not trigger another merge"
    );

    seed_ci_for_head(
        &forge,
        &repo,
        number,
        new_head.clone(),
        3,
        CiJobStatus::Running,
        None,
    );
    assert_eq!(
        block_on(worker.tick(ts("2026-05-29T00:03:00Z"))).expect("running CI is not enough"),
        Progress::unchanged()
    );
    assert_eq!(
        pull_request_state(&forge, &repo, number),
        PullRequestState::Open
    );
    assert_eq!(
        counted.count(CountedForgeOp::MergePullRequest),
        1,
        "running repaired-head CI must not trigger another merge"
    );
    let reviews = block_on(forge.list_pull_request_reviews(&pr_id)).expect("reviews list");
    assert_eq!(
        reviews.len(),
        1,
        "conflict resolution keeps the existing approval"
    );

    seed_successful_ci_for_head(&forge, &repo, number, new_head, 4);
    assert_eq!(
        block_on(worker.tick(ts("2026-05-29T00:04:00Z"))).expect("new green CI lands"),
        Progress {
            changed: true,
            actions: 1,
        }
    );
    assert_eq!(
        pull_request_state(&forge, &repo, number),
        PullRequestState::Merged
    );
    assert_eq!(
        pull_request_labels(&forge, &repo, number),
        vec![
            "alignment".to_string(),
            "implementation".to_string(),
            "landed".to_string(),
        ]
    );
    assert_eq!(counted.count(CountedForgeOp::MergePullRequest), 2);
    assert_eq!(
        counted.count(CountedForgeOp::RequestPullRequestReviewers),
        0,
        "conflict repair and mechanical landing do not request another review"
    );
    let reviews = block_on(forge.list_pull_request_reviews(&pr_id)).expect("reviews list");
    assert_eq!(
        reviews.len(),
        1,
        "mechanical landing did not record a second review"
    );
}

#[test]
fn reference_landing_pr_without_approval_does_not_merge_even_when_ci_passes() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let number = create_pull_request(&forge, &repo, &["implementation", "landing"]);
    let counted = CountingForge::new(forge.clone());
    counted.enable_synthetic_pull_request_heads();
    let head = projected_head(&counted, &repo, number);
    seed_successful_ci_for_head(&forge, &repo, number, head, 1);
    let workflow = temper_testing::workflow();
    let journal = InMemoryJournal::new();
    let worker = MechanicalWorker::new(&workflow, &counted, &repo, &journal, lease_policy());

    assert_eq!(
        block_on(worker.tick(ts("2026-05-29T00:00:00Z"))).expect("approval gate miss is nonfatal"),
        Progress::unchanged()
    );
    assert_eq!(counted.count(CountedForgeOp::MergePullRequest), 0);
    assert_eq!(
        pull_request_state(&forge, &repo, number),
        PullRequestState::Open
    );
}
