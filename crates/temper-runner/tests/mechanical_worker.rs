//! Integration tests for the mechanical controller worker.

use chrono::{DateTime, Duration, Utc};
use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
mod support;

use support::{CountedForgeOp, CountingForge};
use temper_forge::{
    BranchRef, CreateIssue, CreatePullRequest, CreateRepository, Forge, IssueQuery, IssueState,
    ItemListDetails, ItemNumber, MergeMethod, MergePullRequest, PullRequestQuery, PullRequestState,
    PullRequestUpdateState, RepositoryId, UpdateIssue, UpdatePullRequest, UserId,
};
use temper_forge_memory::MemoryForge;
use temper_runner::{MechanicalWorker, Progress, Worker};
use temper_workflow::{
    InMemoryJournal, Lease, LeasePolicy, RawWorkflowSpec, RoleId, WorkflowMetadata,
    parse_metadata_block, render_metadata_block,
};

const FIXTURE: &str = include_str!("../../temper-workflow/fixtures/reference-delivery.json");

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
    create_issue_with_body(forge, repo, labels, String::new())
}

fn create_issue_with_body(
    forge: &MemoryForge,
    repo: &RepositoryId,
    labels: &[&str],
    body: String,
) -> ItemNumber {
    block_on(forge.create_issue(
        repo,
        CreateIssue {
            title: "issue".into(),
            body,
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

fn merge_pull_request(forge: &MemoryForge, repo: &RepositoryId, number: ItemNumber) {
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

fn issue_body(forge: &MemoryForge, repo: &RepositoryId, number: ItemNumber) -> String {
    block_on(forge.get_issue_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("issue exists")
        .body
}

fn expired_lease_body() -> String {
    render_metadata_block(&WorkflowMetadata {
        lease: Some(Lease {
            role: RoleId::new("engineer"),
            worker: "abandoned-worker".into(),
            claimed_at: ts("2026-05-29T00:00:00Z"),
            heartbeat_at: ts("2026-05-29T00:05:00Z"),
            expires_at: ts("2026-05-29T00:10:00Z"),
        }),
        ..WorkflowMetadata::default()
    })
}

// The mechanical scan stays label-bounded with one deliberate exception: the
// reference workflow's default-kind `raw_intake` automation queue has no label
// to filter on, so discovering raw human intake costs a single state-bounded
// open-all issue listing (open + summary, never closed history). See the
// reference-workflow gap record.
fn is_bounded_issue_query(query: &IssueQuery) -> bool {
    query.details == ItemListDetails::summary()
        && match query.state {
            Some(IssueState::Open) => true,
            Some(_) => !query.labels.is_empty(),
            None => false,
        }
}

fn is_bounded_pull_request_query(query: &PullRequestQuery) -> bool {
    query.state.is_some() && !query.labels.is_empty() && query.details == ItemListDetails::summary()
}

fn is_terminal_implementation_query(query: &PullRequestQuery) -> bool {
    matches!(
        query.state,
        Some(PullRequestState::Closed | PullRequestState::Merged)
    ) && has_single_label(&query.labels, "implementation")
}

fn has_single_label(labels: &[String], label: &str) -> bool {
    labels.len() == 1 && labels[0] == label
}

fn mechanical_worker<'a>(
    forge: &'a MemoryForge,
    repo: &'a RepositoryId,
    workflow: &'a temper_workflow::ValidatedWorkflow,
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
fn normal_mechanical_tick_avoids_default_all_history_lists() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    for _ in 0..25 {
        let issue = create_issue(&forge, &repo, &[]);
        close_issue(&forge, &repo, issue);
        let pull_request = create_pull_request(&forge, &repo, &[]);
        close_pull_request(&forge, &repo, pull_request);
    }
    let counted = CountingForge::new(forge.clone());
    let workflow = workflow();
    let journal = InMemoryJournal::new();
    let worker = MechanicalWorker::new(&workflow, &counted, &repo, &journal, lease_policy());

    assert_eq!(
        block_on(worker.tick(ts("2026-05-29T00:00:00Z"))).expect("tick succeeds"),
        Progress::unchanged()
    );
    assert!(counted.count(CountedForgeOp::ListIssues) > 0);
    assert!(counted.count(CountedForgeOp::ListPullRequests) > 0);
    assert!(
        !counted
            .issue_queries()
            .iter()
            .any(|query| query == &IssueQuery::default())
    );
    assert!(
        !counted
            .pull_request_queries()
            .iter()
            .any(|query| query == &PullRequestQuery::default())
    );
    assert!(counted.issue_queries().iter().all(is_bounded_issue_query));
    assert!(
        counted
            .pull_request_queries()
            .iter()
            .all(is_bounded_pull_request_query)
    );
    // The only open-all issue listing is the default-kind `raw_intake`
    // automation queue's single state-bounded scan; everything else is
    // label-bounded and no closed history is listed open-ended.
    let open_all_issue_queries: Vec<IssueQuery> = counted
        .issue_queries()
        .into_iter()
        .filter(|query| query.labels.is_empty())
        .collect();
    assert_eq!(
        open_all_issue_queries.len(),
        1,
        "exactly one default-kind open-all issue listing per mechanical tick"
    );
    assert!(
        open_all_issue_queries
            .iter()
            .all(|query| query.state == Some(IssueState::Open)
                && query.details == ItemListDetails::summary())
    );
}

#[test]
fn normal_mechanical_tick_skips_terminal_implementation_only_pr_history() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    for _ in 0..100 {
        let pull_request = create_pull_request(&forge, &repo, &["implementation"]);
        merge_pull_request(&forge, &repo, pull_request);
    }
    let counted = CountingForge::new(forge.clone());
    let workflow = workflow();
    let journal = InMemoryJournal::new();
    let worker = MechanicalWorker::new(&workflow, &counted, &repo, &journal, lease_policy());

    assert_eq!(
        block_on(worker.tick(ts("2026-05-29T00:00:00Z"))).expect("tick succeeds"),
        Progress::unchanged()
    );
    assert!(
        counted
            .pull_request_queries()
            .iter()
            .any(|query| query.state == Some(PullRequestState::Open)
                && has_single_label(&query.labels, "implementation"))
    );
    assert!(
        !counted
            .pull_request_queries()
            .iter()
            .any(is_terminal_implementation_query)
    );
    assert_eq!(counted.count(CountedForgeOp::GetPullRequestByNumber), 0);
}

#[test]
fn deep_audit_tick_finds_unlabelled_expired_lease() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    // Closed + unlabelled: no bounded query targets it — reconciliation lists by
    // workflow label and the default-kind `raw_intake` automation open-all lists
    // only open issues — so the bounded tick leaves the expired lease untouched
    // and only a deep audit's all-history listing finds it.
    let issue = create_issue_with_body(&forge, &repo, &[], expired_lease_body());
    close_issue(&forge, &repo, issue);
    let counted = CountingForge::new(forge.clone());
    let workflow = workflow();
    let journal = InMemoryJournal::new();
    let worker = MechanicalWorker::new(&workflow, &counted, &repo, &journal, lease_policy());

    assert_eq!(
        block_on(worker.tick(ts("2026-05-29T00:20:00Z"))).expect("bounded tick succeeds"),
        Progress::unchanged()
    );
    assert!(
        parse_metadata_block(&issue_body(&forge, &repo, issue))
            .expect("metadata parses")
            .expect("metadata present")
            .lease
            .is_some()
    );

    assert_eq!(
        block_on(worker.tick_deep_audit(ts("2026-05-29T00:21:00Z"))).expect("deep audit succeeds"),
        Progress {
            changed: true,
            actions: 1,
        }
    );
    assert!(
        counted
            .issue_queries()
            .iter()
            .any(|query| query == &IssueQuery::default())
    );
    assert!(
        counted
            .pull_request_queries()
            .iter()
            .any(|query| query == &PullRequestQuery::default())
    );
    assert!(
        parse_metadata_block(&issue_body(&forge, &repo, issue))
            .expect("metadata parses")
            .expect("metadata present")
            .lease
            .is_none()
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
