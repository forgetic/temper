//! Hermetic daemon/worker lifecycle regression for engineer session resume.
//!
//! This test composes the real daemon transport, real worker loop, real
//! `CodingExecutor`, local `file://` git, `MemoryForge`, and the basic-delivery
//! workflow. It is deliberately in-process (no Forgejo service): MemoryForge is
//! the Forge state, while local bare git proves the worker pushes the PR head
//! branch the daemon assigned.

use std::collections::BTreeMap;
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use temper_forge::{
    CiJob, CiJobConclusion, CiJobId, CiJobStatus, CreateIssue, CreateRepository, Forge, IssueState,
    ItemNumber, PullRequest, PullRequestId, PullRequestQuery, PullRequestState, RepositoryId,
    RepositoryPath, UserId,
};
use temper_forge_memory::MemoryForge;
use temper_protocol_agent::{AgentSessionState, WorkspaceContext, WorkspaceResult};
use temper_protocol_worker::ResultStatus;
use temper_worker::{
    AgentRunError, AgentRunner, CapabilitySpec, CodingExecutor, CodingExecutorConfig,
    ExecutorSelection, PrFreshnessFailure, PrFreshnessGuard, ProgressSink, RoleGitIdentity,
    ScopedWorkspaceCleanupOutcome, WorkerConfig, run_worker_with_transport,
};
use temper_workflow::{InMemoryJournal, LeasePolicy, RawWorkflowSpec, RoleId};

#[path = "support/real_daemon.rs"]
mod real_daemon;
use real_daemon::DaemonHarness;

const BASIC_DELIVERY: &str = include_str!("../../temper-workflow/fixtures/basic-delivery.json");
const ENGINEER: &str = "engineer";
const REPO: &str = "acme/service";

#[test]
fn engineer_session_resumes_after_ci_failure_then_lands_and_cleans_workstream() {
    temper_engine_io::block_on_with(|cx, handle| async move {
        let git = GitFixture::new();
        let forge = Arc::new(MemoryForge::new());
        let repo = create_repo(&forge).await;
        let issue = create_ready_issue(&forge, &repo).await;
        let workflow = Arc::new(workflow());
        let compiled = workflow.compile();
        let role = RoleId::new(ENGINEER);
        let coordination_key = format!("pr-for-code-{}", issue.get());
        let branch = format!("agent/{coordination_key}");

        let applier = Arc::new(temper_engine::LeaseApplier::new(
            forge.clone(),
            LeasePolicy::new(chrono::Duration::seconds(300)),
            "daemon-1",
            Arc::new(temper_engine::ForgeApplier::new(
                forge.clone(),
                workflow.clone(),
            )),
            temper_engine::system_clock(),
        ));
        let mut harness = DaemonHarness::start_with_applier(&handle, applier);

        assert_eq!(
            harness
                .daemon
                .enqueue_scanned_role_work(
                    forge.as_ref(),
                    &repo,
                    workflow.as_ref(),
                    &compiled,
                    ts("2026-05-29T00:00:00Z"),
                    &role,
                    temper_engine::RoleFeedMode::Normal,
                )
                .await
                .expect("code_ready feed succeeds"),
            1,
            "ready code issue should enqueue one engineer implementation job"
        );

        let agent = Arc::new(RecordingAgent::default());
        let executor = Arc::new(
            CodingExecutor::new(
                CodingExecutorConfig {
                    workspace_root: git.workspace_root.clone(),
                    git_base_url: git.git_base_url(),
                    role_identities: role_identities(),
                },
                agent.clone(),
            )
            .with_pr_freshness_guard(Arc::new(DaemonPrFreshnessGuard::new(
                harness.daemon.as_ref().clone(),
            ))),
        );
        let transport = harness.transport();
        let worker_handle = handle.clone();
        handle.spawn(async move {
            let _ = run_worker_with_transport(worker_handle, worker_config(), executor, transport)
                .await;
        });

        let implementation_result = harness.await_result().await;
        assert_eq!(implementation_result.status, ResultStatus::Success);
        assert_eq!(implementation_result.repos.len(), 1);
        assert_eq!(implementation_result.repos[0].branch.name, branch);
        let implementation_head = implementation_result.repos[0].branch.head_sha.clone();
        assert_eq!(git.origin_rev(&branch), implementation_head);

        let mut pulls = wait_for_pull_request_count(&cx, forge.as_ref(), &repo, 1).await;
        let mut pull = pulls.pop().expect("implementation PR exists");
        assert_eq!(pull.source.branch, branch);
        assert_eq!(pull.state, PullRequestState::Open);
        assert!(pull.labels.iter().any(|label| label == "implementation"));
        assert!(pull.labels.iter().any(|label| label == "landing"));
        pull = forge
            .set_pull_request_head(&pull.id, Some(implementation_head.clone()))
            .expect("memory forge observes implementation branch head");

        let runs = agent.runs();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].queue, "code_ready");
        assert_eq!(runs[0].action, "open_pr");
        assert_eq!(runs[0].correlation_key, coordination_key);
        let initial_session = runs[0].session.clone();
        let store = temper_worker::AgentSessionStore::for_workspace_root(
            &git.workspace_root,
            ENGINEER,
            &coordination_key,
        )
        .expect("session store path");
        assert_eq!(
            store.load_sync().expect("saved session loads"),
            Some(initial_session.clone()),
            "successful implementation run should save the engineer session while the PR waits"
        );

        forge.seed_ci_jobs(
            &repo,
            vec![ci_job(
                &repo,
                &pull.id,
                &implementation_head,
                CiJobConclusion::Failure,
                "ci-failed-initial-head",
            )],
        );

        assert_eq!(
            harness
                .daemon
                .enqueue_scanned_role_work(
                    forge.as_ref(),
                    &repo,
                    workflow.as_ref(),
                    &compiled,
                    ts("2026-05-29T00:01:00Z"),
                    &role,
                    temper_engine::RoleFeedMode::Normal,
                )
                .await
                .expect("pr_ci_failed feed succeeds"),
            1,
            "failed CI on the implementation PR should enqueue one PR-feedback job"
        );

        let feedback_result = harness.await_result().await;
        assert_eq!(feedback_result.status, ResultStatus::Success);
        assert!(
            feedback_result.job_id.contains(&format!(
                "/pull_request-{}/engineer/pr_ci_failed",
                pull.number.get()
            )),
            "feedback job id should target the failed implementation PR: {}",
            feedback_result.job_id
        );
        assert_eq!(feedback_result.repos.len(), 1);
        assert_eq!(
            feedback_result.repos[0].branch.name, branch,
            "PR feedback must report the existing PR head branch, not a new branch"
        );
        let feedback_head = feedback_result.repos[0].branch.head_sha.clone();
        assert_ne!(feedback_head, implementation_head);
        assert_eq!(git.origin_rev(&branch), feedback_head);
        assert_eq!(
            git.origin_rev(&format!("{branch}^")),
            implementation_head,
            "the CI fix should be a new commit on top of the implementation head"
        );
        assert_eq!(
            git.origin_log_format(&branch, "%s"),
            format!("Fix CI for {coordination_key}")
        );
        assert_eq!(
            git.origin_show(&format!("{branch}:ci-fix.txt")),
            "fixed failing CI"
        );

        pulls = wait_for_pull_request_count(&cx, forge.as_ref(), &repo, 1).await;
        assert_eq!(pulls[0].number, pull.number);
        assert_eq!(pulls[0].source.branch, branch);
        pull = forge
            .set_pull_request_head(&pull.id, Some(feedback_head.clone()))
            .expect("memory forge observes feedback branch head");

        let runs = agent.runs();
        assert_eq!(
            runs.len(),
            2,
            "only implementation + PR feedback should run"
        );
        let feedback_run = &runs[1];
        assert_eq!(feedback_run.queue, "pr_ci_failed");
        assert_eq!(feedback_run.action, "address_ci_failure");
        assert_eq!(feedback_run.correlation_key, coordination_key);
        assert_eq!(
            feedback_run.branch_hint.as_deref(),
            Some(branch.as_str()),
            "PR feedback checkout should use the existing PR head branch"
        );
        assert_eq!(
            feedback_run.observed_head_sha, implementation_head,
            "PR feedback agent should start from the assigned PR head"
        );
        let freshness = feedback_run
            .pull_request_freshness
            .as_ref()
            .expect("PR feedback context carries freshness facts");
        assert_eq!(freshness.queue_condition.as_deref(), Some("ci_failed"));
        assert_eq!(
            freshness.head_sha.as_deref(),
            Some(implementation_head.as_str()),
            "PR feedback should be assigned against the failed head"
        );
        assert_eq!(freshness.pull_request_id, pull.id.as_str());
        assert_eq!(
            feedback_run.session, initial_session,
            "PR feedback should resume the same engineer agent_session as implementation"
        );
        assert_eq!(
            wait_for_pull_request_count(&cx, forge.as_ref(), &repo, 1)
                .await
                .len(),
            1,
            "feedback success must not open a second pull request"
        );

        wait_for_workstream_inactive(&cx, harness.daemon.as_ref(), &coordination_key).await;
        forge.seed_ci_jobs(
            &repo,
            vec![ci_job(
                &repo,
                &pull.id,
                &feedback_head,
                CiJobConclusion::Success,
                "ci-passed-feedback-head",
            )],
        );

        let cleanup = Arc::new(TestWorkstreamCleaner::new(
            harness.daemon.as_ref().clone(),
            git.workspace_root.clone(),
        ));
        let mechanical_config = temper_engine::MechanicalBackstopConfig {
            repositories: temper_engine::RepositorySet::new(vec![
                temper_engine::RepositoryTarget::new(
                    repo.clone(),
                    RepositoryPath::new("acme", "service"),
                ),
            ]),
            cadence: Duration::from_secs(60),
            lease_policy: LeasePolicy::new(chrono::Duration::seconds(300)),
            pull_request_merge_observer: Some(cleanup.clone()),
        };
        let journals = vec![InMemoryJournal::new()];
        let progress = temper_engine::run_mechanical_backstop_tick(
            forge.as_ref(),
            workflow.as_ref(),
            ts("2026-05-29T00:02:00Z"),
            &mechanical_config,
            &journals,
            &temper_engine::MechanicalScope::All,
        )
        .await
        .expect("mechanical landing tick succeeds");
        assert!(progress.changed, "mechanical tick should land the green PR");

        let landed = forge
            .get_pull_request_by_number(&repo, pull.number)
            .await
            .expect("pull request reload succeeds")
            .expect("pull request still exists");
        assert_eq!(landed.state, PullRequestState::Merged);
        assert!(landed.merge.is_some(), "landing should record a merge");
        assert!(
            !landed.labels.iter().any(|label| label == "landing"),
            "landing label should be removed after merge"
        );
        let source_issue = forge
            .get_issue_by_number(&repo, issue)
            .await
            .expect("issue reload succeeds")
            .expect("source issue exists");
        assert_eq!(source_issue.state, IssueState::Closed);

        let workstream_root =
            temper_worker::scoped_workspace_root(&git.workspace_root, ENGINEER, &coordination_key)
                .expect("scoped workspace path");
        assert_eq!(cleanup.outcomes().len(), 1);
        assert!(
            matches!(
                cleanup.outcomes().first(),
                Some(ScopedWorkspaceCleanupOutcome::Removed { path }) if path == &workstream_root
            ),
            "landing cleanup should remove the inactive engineer workstream: {:?}",
            cleanup.outcomes()
        );
        assert!(
            !workstream_root.exists(),
            "merged PR cleanup should remove the scoped checkout root"
        );
        assert_eq!(
            store.load_sync().expect("session state after cleanup"),
            None,
            "merged PR cleanup should remove the saved agent session state"
        );
    });
}

#[derive(Clone, Debug)]
struct AgentRunRecord {
    queue: String,
    action: String,
    correlation_key: String,
    branch_hint: Option<String>,
    observed_head_sha: String,
    pull_request_freshness: Option<temper_protocol_agent::PullRequestFreshness>,
    session: AgentSessionState,
}

#[derive(Default)]
struct RecordingAgent {
    runs: Mutex<Vec<AgentRunRecord>>,
}

impl RecordingAgent {
    fn runs(&self) -> Vec<AgentRunRecord> {
        self.runs.lock().expect("agent run lock").clone()
    }
}

impl AgentRunner for RecordingAgent {
    async fn run(
        &self,
        context: &WorkspaceContext,
        cwd: &Path,
        _progress: Arc<dyn ProgressSink>,
    ) -> Result<WorkspaceResult, AgentRunError> {
        let primary = context.primary().expect("primary repo in context");
        let repo_cwd = cwd.join(&primary.dir);
        let observed_head_sha = git_output(&["-C", path_str(&repo_cwd), "rev-parse", "HEAD"]);
        let session = context
            .agent_session
            .clone()
            .expect("engineer writable jobs receive an agent session");
        self.runs
            .lock()
            .expect("agent run lock")
            .push(AgentRunRecord {
                queue: context.work_item.queue.clone(),
                action: context.action.clone(),
                correlation_key: context.correlation_key.clone(),
                branch_hint: primary.branch_hint.clone(),
                observed_head_sha,
                pull_request_freshness: context.pull_request_freshness.clone(),
                session,
            });

        let (file, contents, summary) = if context.work_item.queue == "pr_ci_failed" {
            ("ci-fix.txt", "fixed failing CI\n", "fixed CI")
        } else {
            (
                "implementation.txt",
                "implemented the requested change\n",
                "implemented issue",
            )
        };
        fs::write(repo_cwd.join(file), contents).expect("fake agent writes product diff");
        Ok(WorkspaceResult {
            summary: Some(summary.to_string()),
            ..WorkspaceResult::default()
        })
    }
}

struct DaemonPrFreshnessGuard {
    daemon: temper_engine::Daemon,
}

impl DaemonPrFreshnessGuard {
    fn new(daemon: temper_engine::Daemon) -> Self {
        Self { daemon }
    }
}

impl PrFreshnessGuard for DaemonPrFreshnessGuard {
    fn check<'a>(
        &'a self,
        check: &'a temper_protocol_agent::PullRequestFreshness,
    ) -> Pin<Box<dyn Future<Output = Result<(), PrFreshnessFailure>> + Send + 'a>> {
        Box::pin(async move {
            let response = self
                .daemon
                .check_pull_request_freshness(temper_protocol_worker::PullRequestFreshness {
                    repository_id: check.repository_id.clone(),
                    repo: check.repo.clone(),
                    role: check.role.clone(),
                    queue: check.queue.clone(),
                    action: check.action.clone(),
                    number: check.number,
                    pull_request_id: check.pull_request_id.clone(),
                    head_sha: check.head_sha.clone(),
                    queue_condition: check.queue_condition.clone(),
                    queue_labels: check.queue_labels.clone(),
                })
                .await;
            temper_worker::map_pr_freshness_response(response)
        })
    }
}

struct TestWorkstreamCleaner {
    daemon: temper_engine::Daemon,
    workspace_root: PathBuf,
    outcomes: Mutex<Vec<ScopedWorkspaceCleanupOutcome>>,
}

impl TestWorkstreamCleaner {
    fn new(daemon: temper_engine::Daemon, workspace_root: PathBuf) -> Self {
        Self {
            daemon,
            workspace_root,
            outcomes: Mutex::new(Vec::new()),
        }
    }

    fn outcomes(&self) -> Vec<ScopedWorkspaceCleanupOutcome> {
        self.outcomes.lock().expect("cleanup outcome lock").clone()
    }
}

#[async_trait::async_trait]
impl temper_engine::PullRequestMergeObserver for TestWorkstreamCleaner {
    async fn pull_request_merged(&self, pull_request: &PullRequest) {
        let metadata = temper_workflow::parse_metadata_block(&pull_request.body)
            .expect("pull request metadata parses")
            .expect("merged implementation PR carries metadata");
        let coordination_key = metadata
            .correlation_key
            .expect("merged implementation PR carries correlation key");
        let active = self
            .daemon
            .workstream_active_by_correlation_key(&coordination_key)
            .await;
        let outcome = temper_worker::cleanup_scoped_workspace(
            self.workspace_root.clone(),
            ENGINEER.to_string(),
            coordination_key,
            active,
        )
        .await
        .expect("cleanup scoped workspace after merge");
        self.outcomes
            .lock()
            .expect("cleanup outcome lock")
            .push(outcome);
    }
}

struct GitFixture {
    temp: tempfile::TempDir,
    origin: PathBuf,
    workspace_root: PathBuf,
}

impl GitFixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("temp dir");
        let git_root = temp.path().join("git/acme");
        fs::create_dir_all(&git_root).expect("git root");
        let origin = git_root.join("service.git");
        git(&["init", "--bare", path_str(&origin)]);
        seed_origin(&origin, &temp.path().join("seed"));
        Self {
            workspace_root: temp.path().join("workspaces"),
            temp,
            origin,
        }
    }

    fn git_base_url(&self) -> String {
        format!("file://{}/git", path_str(self.temp.path()))
    }

    fn origin_rev(&self, refname: &str) -> String {
        git_output(&["-C", path_str(&self.origin), "rev-parse", refname])
    }

    fn origin_show(&self, spec: &str) -> String {
        git_output(&["-C", path_str(&self.origin), "show", spec])
    }

    fn origin_log_format(&self, refname: &str, format: &str) -> String {
        git_output(&[
            "-C",
            path_str(&self.origin),
            "log",
            "-1",
            &format!("--format={format}"),
            refname,
        ])
    }
}

fn worker_config() -> WorkerConfig {
    WorkerConfig {
        daemon_url: "http://placeholder".to_string(),
        worker_id: "lifecycle-worker".to_string(),
        capabilities: vec![CapabilitySpec {
            repo: REPO.to_string(),
            role: ENGINEER.to_string(),
        }],
        role_identities: role_identities(),
        max_concurrent_jobs: 1,
        poll_wait: Duration::from_millis(20),
        heartbeat_interval: Duration::from_millis(50),
        executor: ExecutorSelection::Stub,
    }
}

fn role_identities() -> BTreeMap<String, RoleGitIdentity> {
    let mut identities = BTreeMap::new();
    identities.insert(
        ENGINEER.to_string(),
        RoleGitIdentity {
            user: "Smith Engineer".to_string(),
            email: "engineer@example.test".to_string(),
            token: "test-token".to_string(),
        },
    );
    identities
}

async fn create_repo(forge: &MemoryForge) -> RepositoryId {
    forge
        .create_repository(CreateRepository {
            owner: "acme".to_string(),
            name: "service".to_string(),
            default_branch: "main".to_string(),
            description: None,
        })
        .await
        .expect("repository is created")
        .id
}

async fn create_ready_issue(forge: &MemoryForge, repo: &RepositoryId) -> ItemNumber {
    forge
        .create_issue(
            repo,
            CreateIssue {
                title: "Ready issue".to_string(),
                body: "Implement the thing.".to_string(),
                labels: vec!["code".to_string(), "ready".to_string()],
                assignees: Vec::<UserId>::new(),
            },
        )
        .await
        .expect("issue is created")
        .number
}

async fn wait_for_pull_request_count(
    cx: &temper_engine_io::Cx,
    forge: &MemoryForge,
    repo: &RepositoryId,
    expected: usize,
) -> Vec<PullRequest> {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        let pulls = forge
            .list_pull_requests(repo, PullRequestQuery::default())
            .await
            .expect("list pull requests succeeds");
        if pulls.len() == expected {
            return pulls;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {expected} pull request(s), saw {}",
            pulls.len()
        );
        temper_engine_io::runtime::sleep_for(cx, Duration::from_millis(10)).await;
    }
}

async fn wait_for_workstream_inactive(
    cx: &temper_engine_io::Cx,
    daemon: &temper_engine::Daemon,
    coordination_key: &str,
) {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if !daemon
            .workstream_active_by_correlation_key(coordination_key)
            .await
        {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for workstream {coordination_key} to become inactive"
        );
        temper_engine_io::runtime::sleep_for(cx, Duration::from_millis(10)).await;
    }
}

fn ci_job(
    repo: &RepositoryId,
    pull_request: &PullRequestId,
    sha: &str,
    conclusion: CiJobConclusion,
    id: &str,
) -> CiJob {
    let timestamp = ts("2026-05-29T00:00:30Z");
    CiJob {
        id: CiJobId::new(id),
        repo_id: repo.clone(),
        pull_request_id: Some(pull_request.clone()),
        commit_sha: sha.to_string(),
        name: "validate".to_string(),
        status: CiJobStatus::Completed,
        conclusion: Some(conclusion),
        url: None,
        created_at: timestamp,
        started_at: Some(timestamp),
        completed_at: Some(timestamp),
        updated_at: timestamp,
    }
}

fn workflow() -> temper_workflow::ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(BASIC_DELIVERY).expect("workflow parses");
    spec.validate().expect("workflow validates")
}

fn ts(value: &str) -> DateTime<Utc> {
    value.parse().expect("valid RFC3339 timestamp")
}

fn seed_origin(origin: &Path, seed: &Path) {
    git(&["init", "-b", "main", path_str(seed)]);
    fs::write(seed.join("README.md"), "# seed\n").expect("seed file");
    git(&[
        "-C",
        path_str(seed),
        "-c",
        "user.name=Seed User",
        "-c",
        "user.email=seed@example.test",
        "add",
        "README.md",
    ]);
    git(&[
        "-C",
        path_str(seed),
        "-c",
        "user.name=Seed User",
        "-c",
        "user.email=seed@example.test",
        "commit",
        "-m",
        "initial commit",
    ]);
    git(&[
        "-C",
        path_str(seed),
        "remote",
        "add",
        "origin",
        path_str(origin),
    ]);
    git(&["-C", path_str(seed), "push", "origin", "main"]);
}

fn git(args: &[&str]) {
    let output = Command::new("git").args(args).output().expect("git");
    assert!(
        output.status.success(),
        "git {args:?} failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_output(args: &[&str]) -> String {
    let output = Command::new("git").args(args).output().expect("git");
    assert!(
        output.status.success(),
        "git {args:?} failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("git output is utf8")
        .trim_end_matches('\n')
        .to_string()
}

fn path_str(path: &Path) -> &str {
    path.as_os_str().to_str().expect("utf8 path")
}
