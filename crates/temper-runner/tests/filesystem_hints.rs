//! Filesystem-backed wake integration tests with distinct handles.

use async_trait::async_trait;
use chrono::Duration;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration as StdDuration, Instant};
use temper_forge::{
    CreateIssue, CreatePullRequestReview, Forge, RepositoryId, ReviewDecision, UpdateIssue, UserId,
};
use temper_forge_filesystem::FilesystemForge;
use temper_runner::{
    broad_targets, Agent, AgentError, Progress, RoleTools, RoleWorker, WakeTarget,
    WakeablePollLoop, WorkItem, Worker,
};
use temper_testing::{block_on, repo_input, runner_config, user, workflow};
use temper_workflow::{ArtifactKindId, QueueId, RoleId, TransitionId};

struct TempRoot {
    path: PathBuf,
}

impl TempRoot {
    fn new(suite: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "temper-runner-filesystem-hints-{suite}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        Self { path }
    }

    fn forge(&self) -> FilesystemForge {
        FilesystemForge::new(&self.path)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

struct NotifyingClaimAgent {
    claimed: Arc<AtomicBool>,
}

#[async_trait]
impl Agent<FilesystemForge> for NotifyingClaimAgent {
    async fn service(
        &self,
        item: &WorkItem,
        tools: &RoleTools<'_, FilesystemForge>,
    ) -> Result<bool, AgentError> {
        if item.queue == QueueId::new("code_ready") && item.kind == ArtifactKindId::new("code") {
            tools
                .run(item.target, &TransitionId::new("claim_code"))
                .await?;
            self.claimed.store(true, Ordering::SeqCst);
            return Ok(true);
        }
        Ok(false)
    }
}

struct CountingWorker {
    ticks: Arc<AtomicU64>,
}

#[async_trait]
impl Worker for CountingWorker {
    async fn tick(
        &self,
        _now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Progress, temper_runner::WorkerError> {
        self.ticks.fetch_add(1, Ordering::SeqCst);
        Ok(Progress::unchanged())
    }

    fn name(&self) -> &str {
        "counting"
    }
}

fn create_repo(forge: &FilesystemForge) -> RepositoryId {
    block_on(forge.create_repository(repo_input()))
        .expect("repository is created")
        .id
}

#[test]
fn distinct_handle_issue_label_mutation_wakes_role_worker_before_poll_deadline() {
    let root = TempRoot::new("role");
    let producer = root.forge();
    let repo = create_repo(&producer);
    let issue = block_on(producer.create_issue(
        &repo,
        CreateIssue {
            title: "implement feature".into(),
            body: String::new(),
            labels: vec!["code".into()],
            assignees: Vec::<UserId>::new(),
        },
    ))
    .expect("issue is created before it is ready");
    let workflow = workflow();
    let compiled = workflow.compile();
    let role = RoleId::new("engineer");
    let worker_forge = root.forge().as_user(user("engineer", "engineer"));
    let claimed = Arc::new(AtomicBool::new(false));
    let worker = RoleWorker::new(
        &workflow,
        &compiled,
        &worker_forge,
        &repo,
        role.clone(),
        Arc::new(NotifyingClaimAgent {
            claimed: Arc::clone(&claimed),
        }),
        runner_config().execution_context(&role),
    );
    let mut hints = worker_forge.subscribe_hints();
    let target = WakeTarget::Role(role.clone());
    let loop_ = WakeablePollLoop::new(&worker, target.clone(), Duration::seconds(30));
    let start = Instant::now();

    std::thread::scope(|scope| {
        let handle = scope.spawn(|| {
            block_on(loop_.run_until(
                &mut hints,
                |_| vec![target.clone()],
                || claimed.load(Ordering::SeqCst),
            ))
        });

        std::thread::sleep(StdDuration::from_millis(50));
        block_on(producer.update_issue(
            &issue.id,
            UpdateIssue {
                add_labels: vec!["ready".into()],
                ..UpdateIssue::default()
            },
        ))
        .expect("issue label is updated");

        let report = handle
            .join()
            .expect("worker thread joins")
            .expect("wake loop runs");
        assert!(report.ticks >= 2);
    });

    assert!(
        start.elapsed() < StdDuration::from_secs(1),
        "hint-driven handoff should beat the 30s poll interval"
    );
}

#[test]
fn pull_request_review_hint_wakes_broad_mechanical_route() {
    let root = TempRoot::new("review-route");
    let writer = root.forge();
    let repo = create_repo(&writer);
    let pr = block_on(writer.create_pull_request(
        &repo,
        temper_testing::pull_request_input(
            &repo,
            "implementation",
            "",
            "feature",
            vec!["implementation".into()],
        ),
    ))
    .expect("pull request is created before subscribing");
    let mut hints = root.forge().subscribe_hints();
    let ticks = Arc::new(AtomicU64::new(0));
    let worker = CountingWorker {
        ticks: Arc::clone(&ticks),
    };
    let target = WakeTarget::Mechanical;
    let loop_ = WakeablePollLoop::new(&worker, target.clone(), Duration::seconds(30));
    let start = Instant::now();

    std::thread::scope(|scope| {
        let handle = scope.spawn(|| {
            block_on(loop_.run_until(
                &mut hints,
                |_| broad_targets([RoleId::new("reviewer")]),
                || ticks.load(Ordering::SeqCst) >= 2,
            ))
        });

        std::thread::sleep(StdDuration::from_millis(50));
        block_on(writer.submit_pull_request_review(
            &pr.id,
            CreatePullRequestReview {
                decision: ReviewDecision::Approved,
                body: None,
            },
        ))
        .expect("review is submitted");

        handle
            .join()
            .expect("worker thread joins")
            .expect("wake loop runs");
    });

    assert!(
        start.elapsed() < StdDuration::from_secs(1),
        "broad review hint should beat the 30s poll interval"
    );
}

#[test]
fn restarted_listener_still_converges_on_next_tick() {
    let root = TempRoot::new("restart");
    let producer = root.forge();
    let repo = create_repo(&producer);
    block_on(producer.create_issue(
        &repo,
        CreateIssue {
            title: "implement feature".into(),
            body: String::new(),
            labels: vec!["code".into(), "ready".into()],
            assignees: Vec::<UserId>::new(),
        },
    ))
    .expect("issue is created before listener restart");

    let workflow = workflow();
    let compiled = workflow.compile();
    let role = RoleId::new("engineer");
    let worker_forge = root.forge().as_user(user("engineer", "engineer"));
    let claimed = Arc::new(AtomicBool::new(false));
    let worker = RoleWorker::new(
        &workflow,
        &compiled,
        &worker_forge,
        &repo,
        role.clone(),
        Arc::new(NotifyingClaimAgent {
            claimed: Arc::clone(&claimed),
        }),
        runner_config().execution_context(&role),
    );
    let mut restarted_hints = worker_forge.subscribe_hints();
    let target = WakeTarget::Role(role.clone());
    let loop_ = WakeablePollLoop::new(&worker, target.clone(), Duration::seconds(30));

    let report = block_on(loop_.run_until(
        &mut restarted_hints,
        |_| vec![target.clone()],
        || claimed.load(Ordering::SeqCst),
    ))
    .expect("initial tick catches pre-existing work even if old hint was missed");

    assert_eq!(report.ticks, 1);
    assert!(claimed.load(Ordering::SeqCst));
}
