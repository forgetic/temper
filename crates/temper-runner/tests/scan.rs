//! Integration tests for Forge-backed runner scans.

use chrono::{DateTime, Utc};
use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use temper_forge::{
    BranchRef, CiJob, CiJobConclusion, CiJobId, CiJobStatus, CreateIssue, CreatePullRequest,
    CreateRepository, Forge, ItemNumber, RepositoryId, UserId,
};
use temper_forge_memory::MemoryForge;
use temper_runner::{scan, scan_role, WorkItem};
use temper_workflow::{ArtifactKindId, ArtifactSource, QueueId, RawWorkflowSpec, RoleId};

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

    assert!(block_on(scan(
        &passing_forge,
        &passing_repo,
        &workflow,
        &compiled,
        now,
    ))
    .expect("scan succeeds")
    .is_empty());
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
