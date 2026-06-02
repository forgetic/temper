//! Integration tests for role workers.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use temper_forge::{CreateIssue, CreateRepository, Forge, ItemNumber, RepositoryId, UserId};
use temper_forge_memory::MemoryForge;
use temper_runner::{Agent, AgentError, Progress, RoleTools, RoleWorker, WorkItem, Worker};
use temper_workflow::{
    ArtifactKindId, ExecutionContext, QueueId, RawWorkflowSpec, RoleId, TransitionId,
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

fn engineer_context() -> ExecutionContext {
    ExecutionContext::new().with_assignee(RoleId::new("engineer"), UserId::new("user-1"))
}

struct FakeClaimer;

#[async_trait]
impl Agent<MemoryForge> for FakeClaimer {
    async fn service(
        &self,
        item: &WorkItem,
        tools: &RoleTools<'_, MemoryForge>,
    ) -> Result<bool, AgentError> {
        if item.queue == QueueId::new("code_ready") && item.kind == ArtifactKindId::new("code") {
            tools
                .run(item.target, &TransitionId::new("claim_code"))
                .await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

fn engineer_worker<'a>(
    forge: &'a MemoryForge,
    repo: &'a RepositoryId,
    workflow: &'a temper_workflow::ValidatedWorkflow,
    compiled: &'a temper_workflow::CompiledWorkflow,
) -> RoleWorker<'a, MemoryForge> {
    RoleWorker::new(
        workflow,
        compiled,
        forge,
        repo,
        RoleId::new("engineer"),
        Arc::new(FakeClaimer),
        engineer_context(),
    )
}

#[test]
fn role_worker_claims_ready_code_issue_once() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let number = create_issue(&forge, &repo, &["code", "ready"]);
    let workflow = workflow();
    let compiled = workflow.compile();
    let worker = engineer_worker(&forge, &repo, &workflow, &compiled);

    assert_eq!(
        block_on(worker.tick(ts("2026-05-29T00:00:00Z"))).expect("tick succeeds"),
        Progress {
            changed: true,
            actions: 1,
        }
    );

    let issue = block_on(forge.get_issue_by_number(&repo, number))
        .expect("issue lookup succeeds")
        .expect("issue exists");
    assert!(issue.labels.contains(&"in-progress".to_string()));
    assert!(!issue.labels.contains(&"ready".to_string()));
    assert!(issue.assignees.contains(&UserId::new("user-1")));

    assert_eq!(
        block_on(worker.tick(ts("2026-05-29T00:00:01Z"))).expect("tick succeeds"),
        Progress::unchanged()
    );
}

#[test]
fn role_worker_tick_with_no_matching_work_is_unchanged() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let workflow = workflow();
    let compiled = workflow.compile();
    let worker = engineer_worker(&forge, &repo, &workflow, &compiled);

    assert_eq!(
        block_on(worker.tick(ts("2026-05-29T00:00:00Z"))).expect("tick succeeds"),
        Progress::unchanged()
    );
}
