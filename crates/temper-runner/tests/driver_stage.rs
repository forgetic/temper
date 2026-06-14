//! Integration tests for runner composition primitives.

use async_trait::async_trait;
use chrono::Duration;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration as StdDuration, Instant};
use temper_forge::{
    ChangeHint, ChangeKind, ChangeSource, ChangeSourceEvent, CreateIssue, CreateRepository, Forge,
    IssueQuery, ItemNumber, RepositoryId, RepositoryPath, User, UserId,
};
use temper_forge_memory::MemoryForge;
use temper_runner::{
    Agent, AgentError, AgentRegistry, BoxError, FixpointDriver, InProcessStage, ManualClock,
    MechanicalWorker, PollLoop, Progress, RoleTools, RoleWorker, RunnerConfig, Scenario,
    WakeTarget, WakeablePollLoop, WorkItem, Worker, run_scenario,
};
use temper_workflow::{
    ArtifactKindId, InMemoryJournal, LeasePolicy, QueueId, RawWorkflowSpec, RoleId, TransitionId,
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

fn workflow() -> temper_workflow::ValidatedWorkflow {
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

struct NotifyingClaimAgent {
    claimed: Arc<AtomicBool>,
}

struct CountingWorker {
    ticks: Arc<AtomicU64>,
    send_on_first_tick: Option<mpsc::Sender<ChangeHint>>,
}

struct ChannelSource {
    receiver: mpsc::Receiver<ChangeHint>,
}

impl ChangeSource for ChannelSource {
    fn recv_timeout(&mut self, timeout: StdDuration) -> ChangeSourceEvent {
        match self.receiver.recv_timeout(timeout) {
            Ok(hint) => ChangeSourceEvent::Hint(hint),
            Err(mpsc::RecvTimeoutError::Timeout) => ChangeSourceEvent::Timeout,
            Err(mpsc::RecvTimeoutError::Disconnected) => ChangeSourceEvent::Closed,
        }
    }

    fn try_recv(&mut self) -> ChangeSourceEvent {
        match self.receiver.try_recv() {
            Ok(hint) => ChangeSourceEvent::Hint(hint),
            Err(mpsc::TryRecvError::Empty) => ChangeSourceEvent::Timeout,
            Err(mpsc::TryRecvError::Disconnected) => ChangeSourceEvent::Closed,
        }
    }
}

#[async_trait]
impl Agent<MemoryForge> for ClaimOnlyAgent {
    async fn service(
        &self,
        item: &WorkItem,
        tools: &RoleTools<'_, MemoryForge>,
    ) -> Result<bool, AgentError> {
        claim_ready_code(item, tools).await.map(|changed| changed.0)
    }
}

#[async_trait]
impl Agent<MemoryForge> for NotifyingClaimAgent {
    async fn service(
        &self,
        item: &WorkItem,
        tools: &RoleTools<'_, MemoryForge>,
    ) -> Result<bool, AgentError> {
        let changed = claim_ready_code(item, tools).await?;
        if changed.0 {
            self.claimed.store(true, Ordering::SeqCst);
        }
        Ok(changed.0)
    }
}

struct Changed(bool);

async fn claim_ready_code(
    item: &WorkItem,
    tools: &RoleTools<'_, MemoryForge>,
) -> Result<Changed, AgentError> {
    if item.queue == QueueId::new("code_ready") && item.kind == ArtifactKindId::new("code") {
        tools
            .run(item.target, &TransitionId::new("claim_code"))
            .await?;
        Ok(Changed(true))
    } else {
        Ok(Changed(false))
    }
}

#[async_trait]
impl Worker for CountingWorker {
    async fn tick(
        &self,
        _now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Progress, temper_runner::WorkerError> {
        let previous = self.ticks.fetch_add(1, Ordering::SeqCst);
        if previous == 0
            && let Some(sender) = &self.send_on_first_tick
        {
            for _ in 0..3 {
                sender.send(test_hint()).expect("hint receiver is alive");
            }
        }
        Ok(Progress::unchanged())
    }

    fn name(&self) -> &str {
        "counting"
    }
}

fn test_hint() -> ChangeHint {
    ChangeHint::item(
        RepositoryPath::new("acme", "service"),
        ItemNumber::new(1),
        ChangeKind::Issue,
    )
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
        temper_runner::RunReport {
            ticks: 1,
            workers: vec![temper_runner::WorkerRunReport {
                name: "mechanical".into(),
                ticks: 1,
                actions: 0,
            }],
        }
    );
}

#[test]
fn wakeable_poll_loop_coalesces_duplicate_burst() {
    let (sender, receiver) = mpsc::channel();
    for _ in 0..3 {
        sender.send(test_hint()).expect("hint queued");
    }
    let ticks = Arc::new(AtomicU64::new(0));
    let worker = CountingWorker {
        ticks: Arc::clone(&ticks),
        send_on_first_tick: None,
    };
    let mut source = ChannelSource { receiver };
    let target = WakeTarget::Mechanical;
    let loop_ = WakeablePollLoop::new(&worker, target.clone(), Duration::seconds(60));

    let report = block_on(loop_.run_until(
        &mut source,
        |_| vec![target.clone()],
        || ticks.load(Ordering::SeqCst) >= 2,
    ))
    .expect("wake loop exits after coalesced wake");

    assert_eq!(report.ticks, 2);
    assert_eq!(ticks.load(Ordering::SeqCst), 2);
}

#[test]
fn wakeable_poll_loop_coalesces_hints_created_during_tick() {
    let (sender, receiver) = mpsc::channel();
    let ticks = Arc::new(AtomicU64::new(0));
    let worker = CountingWorker {
        ticks: Arc::clone(&ticks),
        send_on_first_tick: Some(sender),
    };
    let mut source = ChannelSource { receiver };
    let target = WakeTarget::Mechanical;
    let loop_ = WakeablePollLoop::new(&worker, target.clone(), Duration::seconds(60));

    let report = block_on(loop_.run_until(
        &mut source,
        |_| vec![target.clone()],
        || ticks.load(Ordering::SeqCst) >= 2,
    ))
    .expect("wake loop exits after one follow-up");

    assert_eq!(report.ticks, 2);
    assert_eq!(ticks.load(Ordering::SeqCst), 2);
}

#[test]
fn poll_loop_run_bounded_ticks_single_role_worker() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let workflow = workflow();
    let compiled = workflow.compile();
    block_on(forge.create_issue(
        &repo,
        CreateIssue {
            title: "implement feature".into(),
            body: String::new(),
            labels: vec!["code".into(), "ready".into()],
            assignees: Vec::new(),
        },
    ))
    .expect("issue is created");
    let role = RoleId::new("engineer");
    let engineer_forge = forge.as_user(user("user-engineer", "engineer"));
    let worker = RoleWorker::new(
        &workflow,
        &compiled,
        &engineer_forge,
        &repo,
        role.clone(),
        Arc::new(ClaimOnlyAgent),
        runner_config().execution_context(&role),
    );
    let poll_loop = PollLoop::with_clock(
        &worker as &dyn Worker,
        Duration::seconds(1),
        ManualClock::default(),
    );

    let report = block_on(poll_loop.run_bounded(1)).expect("poll loop ticks once");

    assert_eq!(report.action_count("role:engineer"), Some(1));
    let issues = block_on(forge.list_issues(&repo, IssueQuery::default())).expect("list succeeds");
    let issue = &issues[0];
    assert!(issue.labels.iter().any(|label| label == "in-progress"));
    assert!(!issue.labels.iter().any(|label| label == "ready"));
    assert!(issue.assignees.contains(&UserId::new("user-engineer")));
}

#[test]
fn memory_hint_wakes_role_worker_before_large_poll_deadline() {
    let forge = MemoryForge::new();
    let producer = forge.clone();
    let repo = new_repo(&forge);
    let workflow = workflow();
    let compiled = workflow.compile();
    let role = RoleId::new("engineer");
    let engineer_forge = forge.as_user(user("user-engineer", "engineer"));
    let claimed = Arc::new(AtomicBool::new(false));
    let agent = Arc::new(NotifyingClaimAgent {
        claimed: Arc::clone(&claimed),
    });
    let mut hints = forge.subscribe_hints();
    let worker = RoleWorker::new(
        &workflow,
        &compiled,
        &engineer_forge,
        &repo,
        role.clone(),
        agent,
        runner_config().execution_context(&role),
    );
    let target = WakeTarget::Role(role.clone());
    let loop_ = WakeablePollLoop::new(&worker, target.clone(), Duration::seconds(5));
    let start = Instant::now();

    std::thread::scope(|scope| {
        let handle = scope.spawn(move || {
            block_on(loop_.run_until(
                &mut hints,
                |_| vec![target.clone()],
                || claimed.load(Ordering::SeqCst),
            ))
        });

        std::thread::sleep(StdDuration::from_millis(50));
        block_on(producer.create_issue(
            &repo,
            CreateIssue {
                title: "implement feature".into(),
                body: String::new(),
                labels: vec!["code".into(), "ready".into()],
                assignees: Vec::new(),
            },
        ))
        .expect("issue is created");

        let report = handle
            .join()
            .expect("worker thread joins")
            .expect("wake loop runs");
        assert!(report.ticks >= 2);
    });

    assert!(
        start.elapsed() < StdDuration::from_secs(1),
        "hint-driven handoff should beat the 5s poll interval"
    );
    let issues = block_on(forge.list_issues(&repo, IssueQuery::default())).expect("list succeeds");
    assert!(issues[0].labels.iter().any(|label| label == "in-progress"));
}

#[test]
fn poll_loop_run_until_stops_after_post_tick_signal() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let workflow = workflow();
    let compiled = workflow.compile();
    block_on(forge.create_issue(
        &repo,
        CreateIssue {
            title: "implement feature".into(),
            body: String::new(),
            labels: vec!["code".into(), "ready".into()],
            assignees: Vec::new(),
        },
    ))
    .expect("issue is created");
    let role = RoleId::new("engineer");
    let engineer_forge = forge.as_user(user("user-engineer", "engineer"));
    let worker = RoleWorker::new(
        &workflow,
        &compiled,
        &engineer_forge,
        &repo,
        role.clone(),
        Arc::new(ClaimOnlyAgent),
        runner_config().execution_context(&role),
    );
    let poll_loop = PollLoop::with_clock(
        &worker as &dyn Worker,
        Duration::seconds(1),
        ManualClock::default(),
    );

    // The stop signal is false on the first (pre-tick) check and true on the
    // post-tick check, so the loop performs exactly one tick and shuts down
    // without waiting another poll interval. This is the production-shaped
    // entry point each per-process binary will run.
    let mut ticked = false;
    let report = block_on(poll_loop.run_until(|| {
        let stop = ticked;
        ticked = true;
        stop
    }))
    .expect("poll loop stops after one tick");

    assert_eq!(report.ticks, 1);
    assert_eq!(report.action_count("role:engineer"), Some(1));
    let issues = block_on(forge.list_issues(&repo, IssueQuery::default())).expect("list succeeds");
    assert!(issues[0].labels.iter().any(|label| label == "in-progress"));
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
