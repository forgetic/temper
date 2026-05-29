//! Integration tests for the mechanical controller worker.

use chrono::{DateTime, Duration, Utc};
use harness_forge::{
    CreateIssue, CreateRepository, Forge, IssueState, ItemNumber, RepositoryId, UpdateIssue, UserId,
};
use harness_forge_memory::MemoryForge;
use harness_runner::{MechanicalWorker, Progress, Worker};
use harness_workflow::{InMemoryJournal, LeasePolicy, RawWorkflowSpec};
use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

const FIXTURE: &str = include_str!("../../harness-workflow/fixtures/reference-delivery.json");

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

fn workflow() -> harness_workflow::ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(FIXTURE).expect("fixture parses");
    spec.validate().expect("reference fixture validates")
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

fn issue_labels(forge: &MemoryForge, repo: &RepositoryId, number: ItemNumber) -> Vec<String> {
    let mut labels = block_on(forge.get_issue_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("issue exists")
        .labels;
    labels.sort();
    labels
}

fn mechanical_worker<'a>(
    forge: &'a MemoryForge,
    repo: &'a RepositoryId,
    workflow: &'a harness_workflow::ValidatedWorkflow,
    journal: &'a InMemoryJournal,
) -> MechanicalWorker<'a, MemoryForge, InMemoryJournal> {
    MechanicalWorker::new(workflow, forge, repo, journal, lease_policy())
}

#[test]
fn mechanical_worker_unblocks_resolved_dependency_once() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let dependency = create_issue(&forge, &repo, &["code", "ready"]);
    close_issue(&forge, &repo, dependency);
    let blocked = create_issue(&forge, &repo, &["code", "blocked"]);
    add_issue_dependency(&forge, &repo, blocked, dependency);
    let workflow = workflow();
    let journal = InMemoryJournal::new();
    let worker = mechanical_worker(&forge, &repo, &workflow, &journal);

    assert_eq!(
        block_on(worker.tick(ts("2026-05-29T00:00:00Z"))).expect("tick succeeds"),
        Progress {
            changed: true,
            actions: 1,
        }
    );
    assert_eq!(
        issue_labels(&forge, &repo, blocked),
        vec!["code".to_string(), "ready".to_string()]
    );

    assert_eq!(
        block_on(worker.tick(ts("2026-05-29T00:00:01Z"))).expect("tick succeeds"),
        Progress::unchanged()
    );
}

#[test]
fn mechanical_worker_clean_repo_is_unchanged() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let workflow = workflow();
    let journal = InMemoryJournal::new();
    let worker = mechanical_worker(&forge, &repo, &workflow, &journal);

    assert_eq!(
        block_on(worker.tick(ts("2026-05-29T00:00:00Z"))).expect("tick succeeds"),
        Progress::unchanged()
    );
}

#[test]
fn mechanical_worker_counts_advisory_actions_without_mutating() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let impossible = create_issue(&forge, &repo, &["code", "ready", "in-progress"]);
    let workflow = workflow();
    let journal = InMemoryJournal::new();
    let worker = mechanical_worker(&forge, &repo, &workflow, &journal);

    assert_eq!(
        block_on(worker.tick(ts("2026-05-29T00:00:00Z"))).expect("tick succeeds"),
        Progress::unchanged()
    );
    assert_eq!(worker.advisory_actions(), 1);
    assert_eq!(
        issue_labels(&forge, &repo, impossible),
        vec![
            "code".to_string(),
            "in-progress".to_string(),
            "ready".to_string(),
        ]
    );
}
