//! Filesystem-backed wake integration tests with distinct handles.

use async_trait::async_trait;
use chrono::Duration;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration as StdDuration, Instant};
use temper_forge::{
    CandidateLabelSelection, CandidateLifecycle, CiJob, CiJobConclusion, CiJobId, CiJobStatus,
    CreateIssue, CreatePullRequestReview, Forge, IssueCandidateQuery, ItemListDetails, PullRequest,
    PullRequestCandidateQuery, PullRequestId, PullRequestState, RepositoryId, RequestReviewers,
    ReviewDecision, UpdateIssue, UpdatePullRequest, UserId,
};
use temper_forge_filesystem::FilesystemForge;
use temper_runner::{
    Agent, AgentError, MechanicalWorker, Progress, RoleTools, RoleWorker, WakeTarget,
    WakeablePollLoop, WorkItem, Worker, broad_targets,
};
use temper_testing::{actor_user, block_on, repo_input, runner_config, ts, user, workflow};
use temper_workflow::{
    ArtifactKindId, InMemoryJournal, LeasePolicy, QueueId, RoleId, TransitionId,
};

mod support;
use support::{CountedForgeOp, CountingForge};

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

struct TickObservedWorker<W> {
    inner: W,
    completed_ticks: Arc<AtomicU64>,
}

#[async_trait]
impl<W: Worker> Worker for TickObservedWorker<W> {
    async fn tick(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Progress, temper_runner::WorkerError> {
        let progress = self.inner.tick(now).await?;
        self.completed_ticks.fetch_add(1, Ordering::SeqCst);
        Ok(progress)
    }

    fn name(&self) -> &str {
        self.inner.name()
    }
}

fn create_repo(forge: &FilesystemForge) -> RepositoryId {
    block_on(forge.create_repository(repo_input()))
        .expect("repository is created")
        .id
}

fn lease_policy() -> LeasePolicy {
    LeasePolicy::new(Duration::minutes(30))
}

fn create_implementation_pr(
    forge: &FilesystemForge,
    repo: &RepositoryId,
    labels: Vec<String>,
) -> PullRequest {
    block_on(forge.create_pull_request(
        repo,
        temper_testing::pull_request_input(repo, "implementation", "", "feature", labels),
    ))
    .expect("implementation PR is created")
}

fn request_reviewer(forge: &FilesystemForge, id: &PullRequestId) {
    block_on(forge.request_pull_request_reviewers(
        id,
        RequestReviewers {
            reviewers: vec![UserId::new("reviewer")],
        },
    ))
    .expect("reviewer is requested");
}

fn approve(forge: &FilesystemForge, id: &PullRequestId) {
    let reviewer = forge.as_user(actor_user("reviewer"));
    block_on(reviewer.submit_pull_request_review(
        id,
        CreatePullRequestReview {
            decision: ReviewDecision::Approved,
            body: None,
        },
    ))
    .expect("review is submitted");
}

fn seed_successful_ci(forge: &FilesystemForge, repo: &RepositoryId, pr: &PullRequest) {
    forge
        .seed_ci_jobs(
            repo,
            vec![CiJob {
                id: CiJobId::new(format!("ci-{}", pr.number.get())),
                repo_id: repo.clone(),
                pull_request_id: Some(pr.id.clone()),
                commit_sha: format!("pr-{}-head", pr.number.get()),
                name: "ci".into(),
                status: CiJobStatus::Completed,
                conclusion: Some(CiJobConclusion::Success),
                url: None,
                created_at: ts("2026-05-29T00:00:00Z"),
                started_at: Some(ts("2026-05-29T00:00:30Z")),
                completed_at: Some(ts("2026-05-29T00:01:00Z")),
                updated_at: ts("2026-05-29T00:01:00Z"),
            }],
        )
        .expect("CI jobs are seeded");
}

fn add_landing_label(forge: &FilesystemForge, id: &PullRequestId) {
    block_on(forge.update_pull_request(
        id,
        UpdatePullRequest {
            add_labels: vec!["landing".into()],
            ..UpdatePullRequest::default()
        },
    ))
    .expect("landing label is added");
}

fn pull_request_is_merged(forge: &FilesystemForge, id: &PullRequestId) -> bool {
    block_on(forge.get_pull_request(id))
        .expect("pull request lookup succeeds")
        .is_some_and(|pr| pr.state == PullRequestState::Merged)
}

fn is_bounded_issue_query(query: &IssueCandidateQuery) -> bool {
    query.details == ItemListDetails::summary()
        && (query.lifecycle == CandidateLifecycle::Open
            || matches!(query.labels, CandidateLabelSelection::AnyOf(_)))
}

fn is_bounded_pull_request_query(query: &PullRequestCandidateQuery) -> bool {
    query.details == ItemListDetails::summary()
        && (query.lifecycle == CandidateLifecycle::Open
            || matches!(query.labels, CandidateLabelSelection::AnyOf(_)))
}

#[derive(Clone, Copy)]
enum LandingWake {
    ReviewApproval,
    CiCompletion,
    LandingLabel,
}

impl LandingWake {
    fn suite(self) -> &'static str {
        match self {
            Self::ReviewApproval => "mechanical-review",
            Self::CiCompletion => "mechanical-ci",
            Self::LandingLabel => "mechanical-label",
        }
    }
}

#[test]
fn review_hint_wakes_mechanical_landing_before_poll_deadline() {
    mechanical_landing_wake_driven_by(LandingWake::ReviewApproval);
}

#[test]
fn ci_hint_wakes_mechanical_landing_before_poll_deadline() {
    mechanical_landing_wake_driven_by(LandingWake::CiCompletion);
}

#[test]
fn landing_label_hint_wakes_mechanical_landing_before_poll_deadline() {
    mechanical_landing_wake_driven_by(LandingWake::LandingLabel);
}

fn mechanical_landing_wake_driven_by(final_wake: LandingWake) {
    let root = TempRoot::new(final_wake.suite());
    let writer = root.forge();
    let repo = create_repo(&writer);
    let initial_labels = match final_wake {
        LandingWake::LandingLabel => vec!["implementation".into()],
        LandingWake::ReviewApproval | LandingWake::CiCompletion => {
            vec!["implementation".into(), "landing".into()]
        }
    };
    let pr = create_implementation_pr(&writer, &repo, initial_labels);
    match final_wake {
        LandingWake::ReviewApproval => {
            request_reviewer(&writer, &pr.id);
            seed_successful_ci(&writer, &repo, &pr);
        }
        LandingWake::CiCompletion => {
            request_reviewer(&writer, &pr.id);
            approve(&writer, &pr.id);
        }
        LandingWake::LandingLabel => {
            request_reviewer(&writer, &pr.id);
            approve(&writer, &pr.id);
            seed_successful_ci(&writer, &repo, &pr);
        }
    }

    let mut hints = root.forge().subscribe_hints();
    let counted = CountingForge::new(root.forge());
    let workflow = workflow();
    let journal = InMemoryJournal::new();
    let mechanical = MechanicalWorker::new(&workflow, &counted, &repo, &journal, lease_policy());
    let completed_ticks = Arc::new(AtomicU64::new(0));
    let worker = TickObservedWorker {
        inner: mechanical,
        completed_ticks: completed_ticks.clone(),
    };
    let observer = root.forge();
    let pr_id = pr.id.clone();
    let target = WakeTarget::Mechanical;
    let loop_ = WakeablePollLoop::new(&worker, target.clone(), Duration::seconds(30));
    let mut wake_elapsed = StdDuration::ZERO;

    std::thread::scope(|scope| {
        let handle = scope.spawn(|| {
            block_on(loop_.run_until(
                &mut hints,
                |_| broad_targets([RoleId::new("engineer"), RoleId::new("reviewer")]),
                || pull_request_is_merged(&observer, &pr_id),
            ))
        });

        wait_for_completed_tick(&completed_ticks);
        let wake_start = Instant::now();
        match final_wake {
            LandingWake::ReviewApproval => approve(&writer, &pr.id),
            LandingWake::CiCompletion => seed_successful_ci(&writer, &repo, &pr),
            LandingWake::LandingLabel => add_landing_label(&writer, &pr.id),
        }

        let report = handle
            .join()
            .expect("worker thread joins")
            .expect("wake loop runs");
        wake_elapsed = wake_start.elapsed();
        assert!(report.ticks >= 2);
    });

    assert!(
        wake_elapsed < StdDuration::from_secs(1),
        "landing hint should beat the 30s poll interval"
    );
    assert!(pull_request_is_merged(&root.forge(), &pr.id));
    assert_eq!(counted.count(CountedForgeOp::MergePullRequest), 1);
    assert!(counted.issue_queries().is_empty());
    assert!(counted.pull_request_queries().is_empty());
    assert!(
        counted
            .issue_candidate_queries()
            .iter()
            .all(is_bounded_issue_query)
    );
    assert!(
        counted
            .pull_request_candidate_queries()
            .iter()
            .all(is_bounded_pull_request_query)
    );
}

fn wait_for_completed_tick(completed_ticks: &AtomicU64) {
    let deadline = Instant::now() + StdDuration::from_secs(5);
    while Instant::now() < deadline {
        if completed_ticks.load(Ordering::SeqCst) > 0 {
            return;
        }
        std::thread::sleep(StdDuration::from_millis(5));
    }
    panic!("mechanical worker did not complete its initial tick");
}

#[test]
fn dropped_mechanical_landing_hint_still_converges_by_polling() {
    let root = TempRoot::new("mechanical-poll-backstop");
    let writer = root.forge();
    let repo = create_repo(&writer);
    let pr = create_implementation_pr(
        &writer,
        &repo,
        vec!["implementation".into(), "landing".into()],
    );
    request_reviewer(&writer, &pr.id);
    approve(&writer, &pr.id);

    let mut hints = root.forge().subscribe_hints();
    let workflow = workflow();
    let journal = InMemoryJournal::new();
    let mechanical = MechanicalWorker::new(&workflow, &writer, &repo, &journal, lease_policy());
    let completed_ticks = Arc::new(AtomicU64::new(0));
    let worker = TickObservedWorker {
        inner: mechanical,
        completed_ticks: completed_ticks.clone(),
    };
    let observer = root.forge();
    let pr_id = pr.id.clone();
    let loop_ = WakeablePollLoop::new(&worker, WakeTarget::Mechanical, Duration::milliseconds(150));

    std::thread::scope(|scope| {
        let handle = scope.spawn(|| {
            block_on(loop_.run_until(
                &mut hints,
                |_| Vec::<WakeTarget>::new(),
                || pull_request_is_merged(&observer, &pr_id),
            ))
        });

        wait_for_completed_tick(&completed_ticks);
        seed_successful_ci(&writer, &repo, &pr);

        let report = handle
            .join()
            .expect("worker thread joins")
            .expect("poll loop runs");
        assert!(report.ticks >= 2);
    });

    assert!(pull_request_is_merged(&root.forge(), &pr.id));
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
