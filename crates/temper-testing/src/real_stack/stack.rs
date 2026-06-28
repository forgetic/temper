use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use jig_server::FakeLlm;
use skein::cx::Cx;
use skein::runtime::RuntimeHandle;
use temper_daemon_transport::InProcessTransport;
use temper_engine::{Daemon, RoleFeedMode};
use temper_forge_memory::MemoryForge;
use temper_forge_model::{Forge, ItemNumber, PullRequest, PullRequestQuery, RepositoryId};
use temper_protocol_worker::{JobResult, ResultStatus, WorkerProtocolMessage};
use temper_worker::{CodingExecutor, Transport, WorkerConfig, run_worker_with_transport};
use temper_workflow::{CompiledWorkflow, RoleId, ValidatedWorkflow};

use super::DEFAULT_NOW;
use super::git::{git_output_raw, git_output_trim, path_str};
use super::runner::NativeJigAgentRunner;

/// Built hermetic world. Keep the value alive for as long as worker/agent runs;
/// it owns the temp git remotes/workspaces and the Jig fake LLM server.
pub struct HermeticRealStack {
    pub(crate) _temp: tempfile::TempDir,
    pub(crate) _fake_llm: FakeLlm,
    pub(crate) forge: Arc<MemoryForge>,
    pub(crate) workflow: Arc<ValidatedWorkflow>,
    pub(crate) compiled: CompiledWorkflow,
    pub(crate) daemon: Arc<Daemon>,
    pub(crate) result_tx: temper_engine_io::CqSender<JobResult>,
    pub(crate) result_rx: temper_engine_io::CqReceiver<JobResult>,
    pub(crate) origins: BTreeMap<String, PathBuf>,
    pub(crate) repo_ids: BTreeMap<String, RepositoryId>,
    pub(crate) workspace_root: PathBuf,
    pub(crate) primary_repo_path: String,
    pub(crate) primary_repo_id: RepositoryId,
    pub(crate) issue_number: ItemNumber,
    pub(crate) role: String,
    pub(crate) worker_config: WorkerConfig,
    pub(crate) executor: Arc<CodingExecutor<NativeJigAgentRunner>>,
    pub(crate) worker_started: bool,
}

impl HermeticRealStack {
    /// Repository id for any seeded repo path (`owner/name`).
    pub fn repo_id(&self, repo: &str) -> Option<&RepositoryId> {
        self.repo_ids.get(repo)
    }

    /// Pull requests in any seeded repo.
    pub async fn pull_requests_for_repo(&self, repo: &str) -> Result<Vec<PullRequest>, String> {
        let repo_id = self
            .repo_id(repo)
            .ok_or_else(|| format!("unknown seeded repository `{repo}`"))?;
        self.forge
            .list_pull_requests(repo_id, PullRequestQuery::default())
            .await
            .map_err(|error| format!("list pull requests for {repo}: {error}"))
    }

    /// Waits until a seeded repo has `expected` pull requests.
    pub async fn wait_for_pull_request_count_for_repo(
        &self,
        cx: &Cx,
        repo: &str,
        expected: usize,
        timeout: Duration,
    ) -> Result<Vec<PullRequest>, String> {
        let deadline = Instant::now() + timeout;
        loop {
            let pulls = self.pull_requests_for_repo(repo).await?;
            if pulls.len() == expected {
                return Ok(pulls);
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "timed out after {timeout:?} waiting for {expected} pull request(s) in {repo}, saw {}",
                    pulls.len()
                ));
            }
            temper_engine_io::runtime::sleep_for(cx, Duration::from_millis(10)).await;
        }
    }

    /// Shared in-memory Forge state. Tests can use MemoryForge-specific helpers
    /// such as `seed_ci_jobs` for follow-on scenarios.
    pub fn forge(&self) -> &MemoryForge {
        self.forge.as_ref()
    }

    /// Primary repository id in [`Self::forge`].
    pub fn primary_repo_id(&self) -> &RepositoryId {
        &self.primary_repo_id
    }

    /// Primary repository path (`owner/name`).
    pub fn primary_repo_path(&self) -> &str {
        &self.primary_repo_path
    }

    /// Seeded issue number.
    pub fn issue_number(&self) -> ItemNumber {
        self.issue_number
    }

    /// Worker workspace root. Useful for inspecting session/checkpoint files.
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// Real daemon handle used by the fixture.
    pub fn daemon(&self) -> &Daemon {
        self.daemon.as_ref()
    }

    /// Starts the real worker loop once. The worker runs detached until the
    /// enclosing skein runtime exits.
    pub fn start_worker(&mut self, handle: &RuntimeHandle) {
        if self.worker_started {
            return;
        }
        self.worker_started = true;
        let worker_handle = handle.clone();
        let config = self.worker_config.clone();
        let executor = self.executor.clone();
        let transport = self.transport();
        handle.spawn(async move {
            let _ = run_worker_with_transport(worker_handle, config, executor, transport).await;
        });
    }

    /// Enqueues the currently seeded issue for the builder's primary worker
    /// role by running the daemon's real role feed against the MemoryForge state.
    pub async fn enqueue_scanned_role_work(&self, now: DateTime<Utc>) -> Result<usize, String> {
        self.enqueue_scanned_role_work_for_role(&self.role, now)
            .await
    }

    /// Enqueues the currently seeded issue for a specific role by running the
    /// daemon's real role feed against the MemoryForge state.
    pub async fn enqueue_scanned_role_work_for_role(
        &self,
        role: &str,
        now: DateTime<Utc>,
    ) -> Result<usize, String> {
        let role = RoleId::new(role.to_string());
        self.daemon
            .enqueue_scanned_role_work(
                self.forge.as_ref(),
                &self.primary_repo_id,
                self.workflow.as_ref(),
                &self.compiled,
                now,
                &role,
                RoleFeedMode::Normal,
            )
            .await
            .map_err(|error| format!("enqueue scanned role work: {error}"))
    }

    /// Awaits the next worker result posted to the daemon transport.
    pub async fn await_worker_result(
        &mut self,
        cx: &Cx,
        timeout: Duration,
    ) -> Result<JobResult, String> {
        match skein::time::timeout(
            temper_engine_io::runtime::timer_now(cx),
            timeout,
            Box::pin(self.result_rx.recv()),
        )
        .await
        {
            Ok(Some(result)) => Ok(result),
            Ok(None) => Err("worker result channel closed".to_string()),
            Err(_) => Err(format!(
                "timed out after {timeout:?} waiting for worker result"
            )),
        }
    }

    /// Convenience for the common `code_ready` success path: enqueue one scanned
    /// engineer job, start the worker, wait for its result, then wait for one
    /// implementation PR to appear.
    pub async fn run_open_pr_job(
        &mut self,
        cx: &Cx,
        handle: &RuntimeHandle,
    ) -> Result<HermeticRunResult, String> {
        let prior_pull_count = self.pull_requests().await?.len();
        let enqueued = self.enqueue_scanned_role_work(default_now()?).await?;
        self.start_worker(handle);
        let job_result = self
            .await_worker_result(cx, Duration::from_secs(20))
            .await?;
        if job_result.status != ResultStatus::Success {
            return Ok(HermeticRunResult {
                enqueued_jobs: enqueued,
                job_result,
                pull_requests: self.pull_requests().await?,
            });
        }
        let pull_requests = self
            .wait_for_pull_request_count(cx, prior_pull_count + 1, Duration::from_secs(10))
            .await?;
        Ok(HermeticRunResult {
            enqueued_jobs: enqueued,
            job_result,
            pull_requests,
        })
    }

    /// Lists current pull requests in the primary repo.
    pub async fn pull_requests(&self) -> Result<Vec<PullRequest>, String> {
        self.forge
            .list_pull_requests(&self.primary_repo_id, PullRequestQuery::default())
            .await
            .map_err(|error| format!("list pull requests: {error}"))
    }

    /// Waits until the primary repo has `expected` pull requests.
    pub async fn wait_for_pull_request_count(
        &self,
        cx: &Cx,
        expected: usize,
        timeout: Duration,
    ) -> Result<Vec<PullRequest>, String> {
        let deadline = Instant::now() + timeout;
        loop {
            let pulls = self.pull_requests().await?;
            if pulls.len() == expected {
                return Ok(pulls);
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "timed out after {timeout:?} waiting for {expected} pull request(s), saw {}",
                    pulls.len()
                ));
            }
            temper_engine_io::runtime::sleep_for(cx, Duration::from_millis(10)).await;
        }
    }

    /// Resolves a ref in a seeded bare origin.
    pub fn origin_rev(&self, repo: &str, refname: &str) -> Result<String, String> {
        let origin = self.origin(repo)?;
        git_output_trim(&["-C", path_str(origin)?, "rev-parse", refname])
    }

    /// Reads a file from a branch in a seeded bare origin.
    pub fn origin_file(&self, repo: &str, branch: &str, path: &str) -> Result<String, String> {
        let origin = self.origin(repo)?;
        git_output_raw(&["-C", path_str(origin)?, "show", &format!("{branch}:{path}")])
    }

    /// Reads the most recent commit subjects from a branch in a seeded bare origin.
    pub fn origin_log_subjects(
        &self,
        repo: &str,
        branch: &str,
        max_count: usize,
    ) -> Result<Vec<String>, String> {
        let origin = self.origin(repo)?;
        let max_count = max_count.max(1).to_string();
        let output = git_output_raw(&[
            "-C",
            path_str(origin)?,
            "log",
            branch,
            &format!("--max-count={max_count}"),
            "--format=%s",
        ])?;
        Ok(output.lines().map(str::to_string).collect())
    }

    fn origin(&self, repo: &str) -> Result<&Path, String> {
        self.origins
            .get(repo)
            .map(PathBuf::as_path)
            .ok_or_else(|| format!("unknown seeded repository `{repo}`"))
    }

    fn transport(&self) -> Arc<ResultTappingTransport> {
        Arc::new(ResultTappingTransport {
            inner: InProcessTransport::new(self.daemon.as_ref().clone()),
            result_tx: self.result_tx.clone(),
        })
    }
}

/// Result returned by [`HermeticRealStack::run_open_pr_job`].
#[derive(Debug)]
pub struct HermeticRunResult {
    pub enqueued_jobs: usize,
    pub job_result: JobResult,
    pub pull_requests: Vec<PullRequest>,
}

/// In-process transport wrapper that delegates to the reusable daemon transport
/// and records every worker `Result` message for assertions.
pub struct ResultTappingTransport {
    inner: InProcessTransport,
    result_tx: temper_engine_io::CqSender<JobResult>,
}

impl Transport for ResultTappingTransport {
    fn send(
        &self,
        cx: Cx,
        message: WorkerProtocolMessage,
    ) -> impl Future<Output = Result<Option<WorkerProtocolMessage>, String>> + Send {
        let inner = self.inner.clone();
        let result_tx = self.result_tx.clone();
        async move {
            if let WorkerProtocolMessage::Result(result) = &message {
                let _ = result_tx.send(result.clone());
            }
            inner.send(cx, message).await
        }
    }
}

fn default_now() -> Result<DateTime<Utc>, String> {
    DEFAULT_NOW
        .parse()
        .map_err(|error| format!("parse default timestamp: {error}"))
}
