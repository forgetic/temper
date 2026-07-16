//! Shared fixtures for the coding worker lifecycle integration test.
//!
//! Keeping these helpers out of the test body lets the scenario remain under the
//! repository Rust file-size guard without weakening the end-to-end assertions.

use std::collections::BTreeMap;
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Utc};
use temper_forge::{
    CiJob, CiJobConclusion, CiJobId, CiJobStatus, CreateIssue, CreateRepository, Forge, ItemNumber,
    PullRequest, PullRequestId, PullRequestQuery, RepositoryId, UserId,
};
use temper_forge_memory::MemoryForge;
use temper_protocol_agent::{AgentSessionState, WorkspaceContext, WorkspaceResult};
use temper_worker::{
    AgentRunError, AgentRunOutput, AgentRunner, CapabilitySpec, ExecutorSelection,
    PrFreshnessFailure, PrFreshnessGuard, RoleGitIdentity, ScopedWorkspaceCleanupOutcome,
    WorkerConfig,
};
use temper_workflow::RawWorkflowSpec;

use super::{BASIC_DELIVERY, ENGINEER, REPO};

#[derive(Clone, Debug)]
pub struct AgentRunRecord {
    pub queue: String,
    pub action: String,
    pub correlation_key: String,
    pub branch_hint: Option<String>,
    pub observed_head_sha: String,
    pub pull_request_freshness: Option<temper_protocol_agent::PullRequestFreshness>,
    pub session: AgentSessionState,
}

#[derive(Default)]
pub struct RecordingAgent {
    runs: Mutex<Vec<AgentRunRecord>>,
}

impl RecordingAgent {
    pub fn runs(&self) -> Vec<AgentRunRecord> {
        self.runs.lock().expect("agent run lock").clone()
    }
}

impl AgentRunner for RecordingAgent {
    async fn run(
        &self,
        _job_id: &str,
        context: &WorkspaceContext,
        cwd: &Path,
    ) -> Result<AgentRunOutput, AgentRunError> {
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
        let result = WorkspaceResult {
            summary: Some(summary.to_string()),
            ..WorkspaceResult::default()
        };
        let fingerprint = temper_worker::fingerprint_writable_repos(context, cwd)
            .await
            .map_err(|error| AgentRunError::transient(format!("fingerprint submit: {error}")))?;
        Ok(AgentRunOutput::with_accepted_submit(
            result,
            temper_worker::AcceptedSubmitProof {
                response: temper_protocol_agent::SubmitForPrResponse::accepted(
                    "recording agent submitted",
                ),
                fingerprint,
            },
        ))
    }
}

pub struct DaemonPrFreshnessGuard {
    daemon: temper_engine::Daemon,
}

impl DaemonPrFreshnessGuard {
    pub fn new(daemon: temper_engine::Daemon) -> Self {
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

pub struct TestWorkstreamCleaner {
    daemon: temper_engine::Daemon,
    workspace_root: PathBuf,
    outcomes: Mutex<Vec<ScopedWorkspaceCleanupOutcome>>,
}

impl TestWorkstreamCleaner {
    pub fn new(daemon: temper_engine::Daemon, workspace_root: PathBuf) -> Self {
        Self {
            daemon,
            workspace_root,
            outcomes: Mutex::new(Vec::new()),
        }
    }

    pub fn outcomes(&self) -> Vec<ScopedWorkspaceCleanupOutcome> {
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

pub struct GitFixture {
    temp: tempfile::TempDir,
    origin: PathBuf,
    pub workspace_root: PathBuf,
}

impl GitFixture {
    pub fn new() -> Self {
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

    pub fn git_base_url(&self) -> String {
        format!("file://{}/git", path_str(self.temp.path()))
    }

    pub fn origin_rev(&self, refname: &str) -> String {
        git_output(&["-C", path_str(&self.origin), "rev-parse", refname])
    }

    pub fn origin_show(&self, spec: &str) -> String {
        git_output(&["-C", path_str(&self.origin), "show", spec])
    }

    pub fn origin_log_format(&self, refname: &str, format: &str) -> String {
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

pub fn worker_config() -> WorkerConfig {
    WorkerConfig {
        daemon_url: "http://placeholder".to_string(),
        worker_id: "lifecycle-worker".to_string(),
        worker_pool: None,
        worker_auth: None,
        capabilities: vec![CapabilitySpec {
            repo: REPO.to_string(),
            role: ENGINEER.to_string(),
        }],
        role_identities: role_identities(),
        max_concurrent_jobs: 1,
        poll_wait: Duration::from_millis(20),
        heartbeat_interval: Duration::from_millis(50),
        liveness_limits: Default::default(),
        result_root: ".temper/worker-results".into(),
        agent_traces: Default::default(),
        executor: ExecutorSelection::Stub,
    }
}

pub fn role_identities() -> BTreeMap<String, RoleGitIdentity> {
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

pub async fn create_repo(forge: &MemoryForge) -> RepositoryId {
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

pub async fn create_ready_issue(forge: &MemoryForge, repo: &RepositoryId) -> ItemNumber {
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

pub async fn wait_for_pull_request_count(
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

pub async fn wait_for_workstream_inactive(
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

pub fn ci_job(
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

pub fn workflow() -> temper_workflow::ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(BASIC_DELIVERY).expect("workflow parses");
    spec.validate().expect("workflow validates")
}

pub fn ts(value: &str) -> DateTime<Utc> {
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
