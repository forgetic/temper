//! Integration tests for runner composition primitives.

use async_trait::async_trait;
use chrono::Duration;
use harness_forge::{CreateIssue, CreateRepository, Forge, IssueQuery, RepositoryId, User, UserId};
use harness_forge_memory::MemoryForge;
use harness_runner::{
    run_scenario, Agent, AgentError, AgentRegistry, BoxError, FixpointDriver, InProcessStage,
    MechanicalWorker, Progress, RoleTools, RunnerConfig, Scenario, WorkItem, Worker,
};
use harness_workflow::{
    ArtifactKindId, InMemoryJournal, LeasePolicy, QueueId, RawWorkflowSpec, RoleId, TransitionId,
};
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

fn workflow() -> harness_workflow::ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(FIXTURE).expect("fixture parses");
    spec.validate().expect("reference fixture validates")
}

fn repo_input() -> CreateRepository {
    CreateRepository {
        owner: "acme".into(),
        name: "service".into(),
        default_branch: "main".into(),
        description: None,
    }
}

fn new_repo(forge: &MemoryForge) -> RepositoryId {
    block_on(forge.create_repository(repo_input()))
        .expect("repository is created")
        .id
}

fn user(id: &str, handle: &str) -> User {
    User {
        id: UserId::new(id),
        handle: handle.into(),
        display_name: None,
        email: None,
    }
}

fn boxed_error(message: &str) -> BoxError {
    Box::new(std::io::Error::other(message.to_string()))
}

fn runner_config() -> RunnerConfig {
    RunnerConfig::new(repo_input())
        .with_role_binding(RoleId::new("engineer"), user("user-engineer", "engineer"))
        .with_lease_ttl(Duration::minutes(30))
        .with_poll_interval(Duration::seconds(1))
}

struct ClaimOnlyAgent;

#[async_trait]
impl Agent<MemoryForge> for ClaimOnlyAgent {
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

#[test]
fn fixpoint_driver_over_empty_repo_converges_in_one_tick() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let workflow = workflow();
    let journal = InMemoryJournal::new();
    let worker = MechanicalWorker::new(
        &workflow,
        &forge,
        &repo,
        &journal,
        LeasePolicy::new(Duration::minutes(30)),
    );
    let driver = FixpointDriver::new(vec![&worker as &dyn Worker]);

    let report = block_on(driver.run(10)).expect("driver converges");

    assert_eq!(
        report,
        harness_runner::RunReport {
            ticks: 1,
            workers: vec![harness_runner::WorkerRunReport {
                name: "mechanical".into(),
                ticks: 1,
                actions: 0,
            }],
        }
    );
}

#[test]
fn in_process_stage_runs_claim_only_scenario_to_quiescence() {
    let forge = MemoryForge::new();
    let mut agents = AgentRegistry::new();
    agents.register(RoleId::new("engineer"), ClaimOnlyAgent);
    let stage = block_on(InProcessStage::with_identity(
        forge,
        workflow(),
        runner_config(),
        agents,
        |forge, binding| forge.as_user(binding.user.clone()),
    ))
    .expect("stage builds");
    let scenario = claim_ready_code_scenario();

    let report = block_on(run_scenario(&stage, &scenario)).expect("scenario passes");

    assert_eq!(report.action_count("role:engineer"), Some(1));
}

fn claim_ready_code_scenario() -> Scenario {
    Scenario::new(
        "claim ready code",
        Box::new(|forge, repo| {
            Box::pin(async move {
                forge
                    .create_issue(
                        repo,
                        CreateIssue {
                            title: "implement feature".into(),
                            body: String::new(),
                            labels: vec!["code".into(), "ready".into()],
                            assignees: Vec::new(),
                        },
                    )
                    .await?;
                Ok::<(), BoxError>(())
            })
        }),
        Box::new(|forge, repo| {
            Box::pin(async move {
                let issues = forge.list_issues(repo, IssueQuery::default()).await?;
                let issue = issues
                    .iter()
                    .find(|issue| issue.labels.iter().any(|label| label == "code"))
                    .ok_or_else(|| boxed_error("missing code issue"))?;
                if !issue.labels.iter().any(|label| label == "in-progress") {
                    return Err(boxed_error("code issue was not claimed"));
                }
                if issue.labels.iter().any(|label| label == "ready") {
                    return Err(boxed_error("ready label was not removed"));
                }
                if !issue.assignees.contains(&UserId::new("user-engineer")) {
                    return Err(boxed_error("engineer was not assigned"));
                }
                Ok::<(), BoxError>(())
            })
        }),
    )
}

#[test]
fn progress_record_counts_changed_actions() {
    let mut progress = Progress::unchanged();
    progress.record(false);
    progress.record(true);
    progress.record(true);

    assert_eq!(
        progress,
        Progress {
            changed: true,
            actions: 2,
        }
    );
}
