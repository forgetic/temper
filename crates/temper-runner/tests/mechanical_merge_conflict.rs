//! Mechanical automation fallback coverage for merge conflicts.

mod support;

use chrono::{DateTime, Duration, Utc};
use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use support::{CountedForgeOp, CountingForge};
use temper_forge::{
    BranchRef, CiJob, CiJobConclusion, CiJobId, CiJobStatus, CreatePullRequest, CreateRepository,
    Forge, ItemNumber, PullRequestState, RepositoryId, UserId,
};
use temper_forge_memory::MemoryForge;
use temper_runner::{MechanicalWorker, Progress, Worker};
use temper_workflow::{InMemoryJournal, LeasePolicy, RawWorkflowSpec};

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
        { "kind": "remove_label", "label": "landing" },
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

#[test]
fn mechanical_conflict_fallback_removes_landing_and_stops_retry_loop() {
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
        "removing landing prevents an immediate retry loop"
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
            "merge-conflict".to_string(),
        ]
    );
    assert_eq!(
        pull_request_state(&forge, &repo, ready),
        PullRequestState::Merged
    );
    assert!(pull_request_labels(&forge, &repo, ready).contains(&"landed".to_string()));
}
