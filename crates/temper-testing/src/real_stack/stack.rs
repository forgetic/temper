use std::collections::BTreeMap;
use std::future::Future;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use jig_server::FakeLlm;
use skein::cx::Cx;
use skein::runtime::RuntimeHandle;
use temper_engine::{Daemon, RoleFeedMode};
use temper_forge_memory::MemoryForge;
use temper_forge_model::{Forge, ItemNumber, PullRequest, PullRequestQuery, RepositoryId};
use temper_protocol_worker::{JobResult, ResultStatus, WorkerAuth, WorkerProtocolMessage};
use temper_worker::{
    CodingExecutor, CodingExecutorConfig, Transport, WorkerComponentHandle, WorkerConfig,
    start_worker_with_transport,
};
use temper_workflow::{
    ArtifactSource, CompiledWorkflow, DurableAssignment, LeaseManager, RoleId, ValidatedWorkflow,
    parse_metadata_block,
};

use super::DEFAULT_NOW;
use super::clock::MutableWallClock;
use super::git::{git_output_raw, git_output_trim, path_str};
use super::pause::{PauseHooks, PausePoint};
use super::runner::{DaemonPrFreshnessGuard, NativeJigAgentRunner};

/// Built hermetic stack. Durable state and replaceable process handles are
/// intentionally different values so tests cannot accidentally rebuild the
/// world when restarting a component.
pub struct HermeticRealStack {
    pub(crate) world: HermeticDurableWorld,
    pub(crate) components: HermeticComponentHandles,
}

/// State that survives daemon and worker replacement.
pub struct HermeticDurableWorld {
    pub(crate) _temp: tempfile::TempDir,
    pub(crate) _fake_llm: FakeLlm,
    pub(crate) forge: Arc<MemoryForge>,
    pub(crate) workflow: Arc<ValidatedWorkflow>,
    pub(crate) compiled: CompiledWorkflow,
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
    pub(crate) coding_config: CodingExecutorConfig,
    pub(crate) runner: Arc<NativeJigAgentRunner>,
    pub(crate) clock: MutableWallClock,
    pub(crate) hooks: PauseHooks,
    pub(crate) router: Arc<DaemonRouter>,
    pub(crate) apply_grace: Option<Duration>,
}

/// Process-local handles that may be stopped and reconstructed over one world.
pub struct HermeticComponentHandles {
    pub(crate) daemon: Arc<Daemon>,
    pub(crate) executor: Arc<CodingExecutor<NativeJigAgentRunner>>,
    pub(crate) worker: Option<WorkerComponentHandle>,
    pub(crate) recovered: BTreeMap<String, HermeticRecoveredClaim>,
}

pub(crate) struct HermeticRecoveredClaim {
    repo: RepositoryId,
    target: ArtifactSource,
    assignment: DurableAssignment,
}

impl Deref for HermeticRealStack {
    type Target = HermeticDurableWorld;

    fn deref(&self) -> &Self::Target {
        &self.world
    }
}

impl DerefMut for HermeticRealStack {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.world
    }
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

    /// Worker workspace root. Useful for inspecting session files.
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// Mutable wall clock shared by every daemon incarnation.
    pub fn clock(&self) -> &MutableWallClock {
        &self.clock
    }

    /// Named, channel-backed pause hooks for deterministic crash placement.
    pub fn pause_hooks(&self) -> &PauseHooks {
        &self.hooks
    }

    /// Real daemon handle used by the current fixture component.
    pub fn daemon(&self) -> &Daemon {
        self.components.daemon.as_ref()
    }

    /// Starts the real worker loop once and retains explicit crash/join control.
    pub fn start_worker(&mut self, handle: &RuntimeHandle) {
        if self.components.worker.is_some() {
            return;
        }
        let transport = self.transport();
        self.components.worker = Some(start_worker_with_transport(
            handle.clone(),
            self.worker_config.clone(),
            self.components.executor.clone(),
            transport,
        ));
    }

    /// Abruptly stops and joins the worker machine. Durable workspaces and its
    /// stable worker identity remain in the world for [`start_worker`](Self::start_worker).
    pub async fn crash_worker(&mut self) {
        if let Some(worker) = self.components.worker.take() {
            worker.crash().await;
        }
    }

    /// Abruptly stops the daemon and installs a fresh daemon over the same
    /// Forge, clock, workflow, journal-facing applier, and in-process endpoint.
    /// The new daemon starts behind its recovery barrier.
    pub async fn replace_daemon(&mut self, handle: &RuntimeHandle) {
        self.components.daemon.crash().await;
        let daemon = self.build_daemon(handle).begin_startup_recovery();
        let daemon = Arc::new(daemon);
        let recovered = self.stage_primary_assignment(daemon.as_ref()).await;
        self.router.replace(daemon.clone());
        self.components.daemon = daemon;
        self.components.recovered = recovered;
        self.components.executor = Arc::new(
            CodingExecutor::new(self.coding_config.clone(), self.runner.clone())
                .with_pr_freshness_guard(Arc::new(DaemonPrFreshnessGuard::new(
                    self.components.daemon.clone(),
                ))),
        );
    }

    /// Opens the current daemon's startup barrier at a named deterministic
    /// pause point. Startup inventory can be staged explicitly by a scenario
    /// before calling this method.
    pub async fn open_recovery_barrier(&mut self) -> Vec<temper_engine::RecoveredJob> {
        self.hooks.reach(PausePoint::RecoveryBarrierOpening).await;
        let orphaned = self.components.daemon.collect_startup_orphans().await;
        let policy = temper_workflow::LeasePolicy::new(chrono::Duration::seconds(300));
        for orphan in &orphaned {
            let claim = self
                .components
                .recovered
                .get(&orphan.job_id)
                .expect("hermetic orphan has durable context");
            LeaseManager::new(self.forge.as_ref(), policy)
                .rollback_assignment(&claim.repo, claim.target, &claim.assignment)
                .await
                .expect("hermetic orphan convergence");
        }
        self.components.daemon.complete_startup_recovery().await;
        self.components.recovered.clear();
        orphaned
    }

    async fn stage_primary_assignment(
        &self,
        daemon: &Daemon,
    ) -> BTreeMap<String, HermeticRecoveredClaim> {
        let mut recovered = BTreeMap::new();
        let issue = self
            .forge
            .get_issue_by_number(&self.primary_repo_id, self.issue_number)
            .await
            .expect("hermetic startup issue inventory");
        let Some(issue) = issue else {
            return recovered;
        };
        let metadata = parse_metadata_block(&issue.body)
            .expect("hermetic startup metadata parses")
            .unwrap_or_default();
        let Some(assignment) = metadata.assignment else {
            return recovered;
        };
        let job_id = assignment
            .job_id
            .clone()
            .filter(|value| !value.trim().is_empty())
            .expect("durable assignment has job id");
        let worker_id = assignment
            .worker_id
            .clone()
            .filter(|value| !value.trim().is_empty())
            .expect("durable assignment has worker id");
        let prior_boot = assignment
            .daemon_boot_id
            .clone()
            .filter(|value| !value.trim().is_empty())
            .expect("durable assignment has daemon boot id");
        let target = ArtifactSource::Issue {
            number: self.issue_number,
        };
        let job = temper_engine::recovered_job_from_assignment(
            self.forge.as_ref(),
            &self.primary_repo_id,
            target,
            &assignment,
            self.workflow.as_ref(),
            &self.compiled,
        )
        .await
        .expect("hermetic durable assignment reconstructs");
        daemon
            .stage_recovered_job(
                temper_engine::RecoveredJob {
                    job_id: job.job_id,
                    worker_id,
                    role: job.role,
                    repo: job.repo,
                    artifact: job.artifact,
                    job_payload: job.job_payload,
                },
                prior_boot,
            )
            .await
            .expect("hermetic durable assignment stages");
        recovered.insert(
            job_id,
            HermeticRecoveredClaim {
                repo: self.primary_repo_id.clone(),
                target,
                assignment,
            },
        );
        recovered
    }

    fn build_daemon(&self, handle: &RuntimeHandle) -> Daemon {
        let applier = Arc::new(temper_engine::LeaseApplier::new(
            self.forge.clone(),
            temper_workflow::LeasePolicy::new(chrono::Duration::seconds(300)),
            "hermetic-daemon",
            Arc::new(
                temper_engine::ForgeApplier::new(self.forge.clone(), self.workflow.clone())
                    .with_child_issue_hook(Arc::new(self.hooks.clone())),
            ),
            self.clock.capability(),
        ));
        let daemon = Daemon::with_applier(Arc::new(handle.clone()), applier);
        match self.apply_grace {
            Some(grace) => daemon.with_apply_grace(grace),
            None => daemon,
        }
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
        self.components
            .daemon
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
            router: self.router.clone(),
            result_tx: self.result_tx.clone(),
            hooks: self.hooks.clone(),
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

/// Replaceable in-process endpoint. Existing workers resolve the current daemon
/// on every protocol message, matching an external worker reconnecting to a
/// restarted daemon at a stable URL.
pub struct DaemonRouter {
    daemon: Mutex<Arc<Daemon>>,
}

impl DaemonRouter {
    pub(crate) fn new(daemon: Arc<Daemon>) -> Self {
        Self {
            daemon: Mutex::new(daemon),
        }
    }

    pub(crate) fn replace(&self, daemon: Arc<Daemon>) {
        *self.daemon.lock().expect("daemon router lock") = daemon;
    }

    fn current(&self) -> Arc<Daemon> {
        self.daemon.lock().expect("daemon router lock").clone()
    }
}

/// In-process transport wrapper that delegates through the replaceable daemon
/// endpoint and records every worker `Result` message for assertions.
pub struct ResultTappingTransport {
    router: Arc<DaemonRouter>,
    result_tx: temper_engine_io::CqSender<JobResult>,
    hooks: PauseHooks,
}

impl Transport for ResultTappingTransport {
    fn send(
        &self,
        _cx: Cx,
        message: WorkerProtocolMessage,
        auth: Option<WorkerAuth>,
    ) -> impl Future<Output = Result<Option<WorkerProtocolMessage>, String>> + Send {
        let daemon = self.router.current();
        let result_tx = self.result_tx.clone();
        let hooks = self.hooks.clone();
        async move {
            let recorded = match &message {
                WorkerProtocolMessage::Result(result) => Some(result.clone()),
                _ => None,
            };
            if recorded.is_some() {
                // A successful coding executor has already committed and pushed
                // before publishing Result.
                hooks.reach(PausePoint::WorkerPushCompleted).await;
                hooks.reach(PausePoint::ResultApplicationStarted).await;
            }
            let reply = daemon
                .deliver_protocol_message_with_auth(message, auth)
                .await;
            if matches!(&reply, Ok(Some(WorkerProtocolMessage::Assign(_)))) {
                // The daemon only emits Assign after the durable claim CAS.
                hooks.reach(PausePoint::AssignmentClaimCommitted).await;
            }
            if let Some(result) = recorded {
                if reply.is_ok() {
                    hooks.reach(PausePoint::ResultApplicationCompleted).await;
                }
                let _ = result_tx.send(result);
            }
            reply
        }
    }
}

fn default_now() -> Result<DateTime<Utc>, String> {
    DEFAULT_NOW
        .parse()
        .map_err(|error| format!("parse default timestamp: {error}"))
}
