//! Live mechanical durable-assignment convergence tests.

use chrono::{DateTime, Duration, Utc};
use std::future::Future;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use temper_forge::{
    CreateIssue, CreateRepository, Forge, ItemNumber, RepositoryId, UpdateIssue, UserId,
};
use temper_forge_memory::{FaultOp, MemoryForge};
use temper_runner::{MechanicalWorker, Progress, Worker};
use temper_workflow::{
    ArtifactKindId, ArtifactSnapshot, ArtifactSource, AssignmentConvergenceOutcome,
    AssignmentConverger, DurableAssignment, InMemoryJournal, Lease, LeasePolicy, RawWorkflowSpec,
    RoleId, WorkflowMetadata, parse_metadata_block, render_metadata_block,
};
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone, Default)]
struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

impl Write for SharedBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for SharedBuffer {
    type Writer = SharedBuffer;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn capture_logs<T>(run: impl FnOnce() -> T) -> (T, Vec<serde_json::Value>) {
    let buffer = SharedBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_writer(buffer.clone())
        .with_max_level(tracing::Level::TRACE)
        .finish();
    let outcome = tracing::subscriber::with_default(subscriber, run);
    let bytes = buffer.0.lock().unwrap().clone();
    let events = String::from_utf8(bytes)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    (outcome, events)
}

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
        Poll::Pending => panic!("in-memory forge futures should not park"),
    }
}

fn ts(value: &str) -> DateTime<Utc> {
    value.parse().expect("valid timestamp")
}

fn workflow() -> temper_workflow::ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(FIXTURE).expect("fixture parses");
    spec.validate().expect("fixture validates")
}

fn new_repo(forge: &MemoryForge) -> RepositoryId {
    block_on(forge.create_repository(CreateRepository {
        owner: "acme".into(),
        name: "service".into(),
        default_branch: "main".into(),
        description: None,
    }))
    .unwrap()
    .id
}

fn expired_assignment(job_id: &str, expires_at: &str) -> (DurableAssignment, WorkflowMetadata) {
    let expires_at = ts(expires_at);
    let assignment = DurableAssignment {
        job_id: Some(job_id.to_string()),
        role: Some(RoleId::new("engineer")),
        queue: Some("code_ready".to_string()),
        action: Some("open_pr".to_string()),
        worker_id: Some("worker-a".to_string()),
        coordination_key: Some("pr-for-code-recovery".to_string()),
        daemon_boot_id: Some("boot-a".to_string()),
        pre_claim_labels: vec!["code".to_string(), "ready".to_string()],
        assigned_at: Some(ts("2026-05-29T00:00:00Z")),
        expires_at: Some(expires_at),
        ..DurableAssignment::default()
    };
    let metadata = WorkflowMetadata {
        kind: Some(ArtifactKindId::new("code")),
        assignment: Some(assignment.clone()),
        lease: Some(Lease {
            role: RoleId::new("engineer"),
            worker: "boot-a".to_string(),
            claimed_at: ts("2026-05-29T00:00:00Z"),
            heartbeat_at: ts("2026-05-29T00:05:00Z"),
            expires_at,
        }),
        ..WorkflowMetadata::default()
    };
    (assignment, metadata)
}

fn create_claimed_issue(
    forge: &MemoryForge,
    repo: &RepositoryId,
    metadata: &WorkflowMetadata,
) -> temper_forge::Issue {
    block_on(forge.create_issue(
        repo,
        CreateIssue {
            title: "recover abandoned issue".to_string(),
            body: render_metadata_block(metadata),
            labels: vec![
                "code".to_string(),
                "in-progress".to_string(),
                "priority-high".to_string(),
            ],
            assignees: vec![UserId::new("engineer")],
        },
    ))
    .unwrap()
}

fn issue_body(forge: &MemoryForge, repo: &RepositoryId, number: ItemNumber) -> String {
    block_on(forge.get_issue_by_number(repo, number))
        .unwrap()
        .unwrap()
        .body
}

fn worker<'a>(
    forge: &'a MemoryForge,
    repo: &'a RepositoryId,
    workflow: &'a temper_workflow::ValidatedWorkflow,
    journal: &'a InMemoryJournal,
) -> MechanicalWorker<'a, MemoryForge, InMemoryJournal> {
    MechanicalWorker::new(
        workflow,
        forge,
        repo,
        journal,
        LeasePolicy::new(Duration::minutes(30)),
    )
}

#[test]
fn direct_assignment_convergence_preserves_an_already_parked_assignment() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let (assignment, metadata) = expired_assignment("job-parked", "2026-05-29T00:10:00Z");
    let issue = create_claimed_issue(&forge, &repo, &metadata);
    block_on(forge.update_issue(
        &issue.id,
        UpdateIssue {
            add_labels: vec!["needs-human".to_string()],
            ..UpdateIssue::default()
        },
    ))
    .expect("issue is parked");
    let workflow = workflow();
    let converger =
        AssignmentConverger::new(&workflow, &forge, LeasePolicy::new(Duration::minutes(30)));

    assert_eq!(
        block_on(converger.converge(
            &repo,
            ArtifactSource::Issue {
                number: issue.number,
            },
            &assignment,
        ))
        .expect("parked convergence is inert"),
        AssignmentConvergenceOutcome::Quarantined
    );
    let parked = block_on(forge.get_issue_by_number(&repo, issue.number))
        .expect("lookup succeeds")
        .expect("issue remains present");
    assert!(parked.labels.contains(&"needs-human".to_string()));
    let parked_metadata = parse_metadata_block(&parked.body)
        .expect("metadata parses")
        .expect("metadata remains present");
    assert_eq!(parked_metadata.assignment.as_ref(), Some(&assignment));
    assert!(parked_metadata.lease.is_some());
}

#[test]
fn live_reconciliation_converges_full_issue_assignment_once() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let (_, metadata) = expired_assignment("job-live-recovery", "2026-05-29T00:10:00Z");
    let issue = create_claimed_issue(&forge, &repo, &metadata);
    let workflow = workflow();
    let journal = InMemoryJournal::new();
    let worker = worker(&forge, &repo, &workflow, &journal);

    assert_eq!(
        block_on(worker.tick(ts("2026-05-29T00:20:00Z"))).unwrap(),
        Progress {
            changed: true,
            actions: 1,
        }
    );
    let recovered = block_on(forge.get_issue_by_number(&repo, issue.number))
        .unwrap()
        .unwrap();
    let mut labels = recovered.labels.clone();
    labels.sort();
    assert_eq!(labels, vec!["code", "priority-high", "ready"]);
    assert!(recovered.assignees.is_empty());
    let metadata = parse_metadata_block(&recovered.body).unwrap().unwrap();
    assert!(metadata.assignment.is_none() && metadata.lease.is_none());
    assert_eq!(
        block_on(worker.tick(ts("2026-05-29T00:21:00Z"))).unwrap(),
        Progress::unchanged()
    );
}

#[test]
fn live_reconciliation_restores_blocked_state_for_unresolved_dependency() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let dependency = block_on(forge.create_issue(
        &repo,
        CreateIssue {
            title: "unfinished prerequisite".to_string(),
            body: String::new(),
            labels: vec!["code".to_string(), "ready".to_string()],
            assignees: Vec::new(),
        },
    ))
    .unwrap();
    let (_, metadata) = expired_assignment("job-blocked", "2026-05-29T00:10:00Z");
    let issue = create_claimed_issue(&forge, &repo, &metadata);
    block_on(forge.add_issue_dependency(&issue.id, dependency.number)).unwrap();
    let workflow = workflow();
    let journal = InMemoryJournal::new();
    let worker = worker(&forge, &repo, &workflow, &journal);

    assert!(
        block_on(worker.tick(ts("2026-05-29T00:20:00Z")))
            .unwrap()
            .changed
    );
    let recovered = block_on(forge.get_issue_by_number(&repo, issue.number))
        .unwrap()
        .unwrap();
    let mut labels = recovered.labels;
    labels.sort();
    assert_eq!(labels, vec!["blocked", "code", "priority-high"]);
    let metadata = parse_metadata_block(&recovered.body).unwrap().unwrap();
    assert!(metadata.assignment.is_none() && metadata.lease.is_none());
}

#[test]
fn live_reconciliation_retries_release_after_forge_outage() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let (_, metadata) = expired_assignment("job-outage", "2026-05-29T00:10:00Z");
    let issue = create_claimed_issue(&forge, &repo, &metadata);
    let workflow = workflow();
    let journal = InMemoryJournal::new();
    let worker = worker(&forge, &repo, &workflow, &journal);

    forge.fail_next(FaultOp::UpdateIssue, "Forge unavailable during release");
    let (failed, events) = capture_logs(|| block_on(worker.tick(ts("2026-05-29T00:20:00Z"))));
    assert!(failed.is_err());
    let convergence = events
        .iter()
        .find(|event| event["fields"]["event"] == "assignment.convergence")
        .expect("unreconciled convergence warning");
    assert_eq!(convergence["level"], "WARN");
    assert_eq!(convergence["fields"]["convergence_result"], "unreconciled");
    assert_eq!(convergence["fields"]["claim_converged"], false);
    assert_eq!(convergence["fields"]["job_id"], "job-outage");
    let claimed = parse_metadata_block(&issue_body(&forge, &repo, issue.number))
        .unwrap()
        .unwrap();
    assert!(claimed.assignment.is_some() && claimed.lease.is_some());

    assert!(
        block_on(worker.tick(ts("2026-05-29T00:21:00Z")))
            .unwrap()
            .changed
    );
    let recovered = parse_metadata_block(&issue_body(&forge, &repo, issue.number))
        .unwrap()
        .unwrap();
    assert!(recovered.assignment.is_none() && recovered.lease.is_none());
}

#[test]
fn live_reconciliation_quarantines_invalid_assignment_contract_once() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let (mut assignment, mut metadata) = expired_assignment("job-invalid", "2026-05-29T00:10:00Z");
    assignment.queue = Some("missing_queue".to_string());
    metadata.assignment = Some(assignment);
    let issue = create_claimed_issue(&forge, &repo, &metadata);
    let workflow = workflow();
    let journal = InMemoryJournal::new();
    let worker = worker(&forge, &repo, &workflow, &journal);

    assert!(
        block_on(worker.tick(ts("2026-05-29T00:20:00Z")))
            .unwrap()
            .changed
    );
    let quarantined = block_on(forge.get_issue_by_number(&repo, issue.number))
        .unwrap()
        .unwrap();
    assert!(quarantined.labels.contains(&"needs-human".to_string()));
    assert!(!quarantined.labels.contains(&"in-progress".to_string()));
    let metadata = parse_metadata_block(&quarantined.body).unwrap().unwrap();
    assert!(metadata.assignment.is_none() && metadata.lease.is_none());
    assert_eq!(
        block_on(forge.list_issue_comments(&quarantined.id))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        block_on(worker.tick(ts("2026-05-29T00:21:00Z"))).unwrap(),
        Progress::unchanged()
    );
}

#[test]
fn stale_assignment_finding_cannot_clear_a_newer_claim() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let (old_assignment, old_metadata) = expired_assignment("job-old", "2026-05-29T00:10:00Z");
    let issue = create_claimed_issue(&forge, &repo, &old_metadata);
    let stale_snapshot = ArtifactSnapshot::from_issue(&issue);
    let (new_assignment, new_metadata) = expired_assignment("job-new", "2026-05-29T00:11:00Z");
    block_on(forge.update_issue(
        &issue.id,
        UpdateIssue {
            body: Some(render_metadata_block(&new_metadata)),
            ..UpdateIssue::default()
        },
    ))
    .unwrap();
    let workflow = workflow();
    let journal = InMemoryJournal::new();
    let worker = worker(&forge, &repo, &workflow, &journal);

    assert_eq!(
        block_on(
            worker.reconcile_targeted_snapshots(ts("2026-05-29T00:20:00Z"), vec![stale_snapshot],)
        )
        .unwrap(),
        Progress::unchanged()
    );
    let current = parse_metadata_block(&issue_body(&forge, &repo, issue.number))
        .unwrap()
        .unwrap();
    assert_eq!(current.assignment, Some(new_assignment));
    assert!(current.lease.is_some());
    assert_ne!(current.assignment, Some(old_assignment));
}
