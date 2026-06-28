use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use skein::runtime::RuntimeHandle;
use temper_agent::{
    CheckpointHook, CodingAgentError, ProviderConfig, WorkspaceContext, WorkspaceResult,
    run_coding_agent_native_with_hooks,
};
use temper_engine::Daemon;
use temper_protocol_agent::{PROTOCOL_VERSION, PullRequestFreshness, StepProgress, StepState};
use temper_worker::{
    AgentRunError, AgentRunner, PrFreshnessFailure, PrFreshnessGuard, ProgressSink,
};

/// In-process native coding-agent runner used by the hermetic real-stack
/// builder. It keeps the worker's real [`temper_worker::CodingExecutor`] in
/// place while pointing the native agent at a Jig-backed provider.
#[derive(Clone)]
pub struct NativeJigAgentRunner {
    pub(crate) handle: RuntimeHandle,
    pub(crate) provider: ProviderConfig,
    pub(crate) max_iterations: usize,
    pub(crate) config_dir: Option<PathBuf>,
    pub(crate) enable_subagents: bool,
    pub(crate) enable_checkpoints: bool,
}

impl AgentRunner for NativeJigAgentRunner {
    async fn run(
        &self,
        context: &WorkspaceContext,
        cwd: &Path,
        progress: Arc<dyn ProgressSink>,
    ) -> Result<WorkspaceResult, AgentRunError> {
        let role = context.work_item.role.clone();
        let correlation_key = context.correlation_key.clone();
        progress.report(StepProgress {
            correlation_key: correlation_key.clone(),
            step: 1,
            status: format!("start {role} run"),
            state: StepState::Started,
            pushed_sha: None,
            note: Some(format!("protocol v{PROTOCOL_VERSION} (native jig)")),
        });

        let checkpoint = self.enable_checkpoints.then(|| {
            Arc::new(HermeticCheckpointer::new(
                context,
                cwd,
                Arc::clone(&progress),
            ))
        });
        let checkpoint_hook = checkpoint
            .as_ref()
            .map(|hook| Arc::clone(hook) as Arc<dyn CheckpointHook>);

        let result = run_coding_agent_native_with_hooks(
            self.handle.clone(),
            &self.provider,
            context,
            cwd,
            self.max_iterations,
            self.config_dir.as_deref(),
            self.enable_subagents,
            None,
            None,
            checkpoint_hook,
        )
        .await
        .map(|(result, _totals)| result)
        .map_err(agent_error);

        if let Ok(result) = &result {
            let step = checkpoint.as_ref().map_or(2, |hook| hook.next_step());
            progress.report(StepProgress {
                correlation_key,
                step,
                status: format!("finish {role} run"),
                state: StepState::Done,
                pushed_sha: None,
                note: result.summary.clone(),
            });
        }

        result
    }
}

#[derive(Clone, Debug)]
struct HermeticCheckpointRepo {
    dir: String,
    branch: String,
}

struct HermeticCheckpointer {
    cwd: PathBuf,
    repos: Vec<HermeticCheckpointRepo>,
    correlation_key: String,
    progress: Arc<dyn ProgressSink>,
    step: AtomicU32,
}

impl HermeticCheckpointer {
    fn new(context: &WorkspaceContext, cwd: &Path, progress: Arc<dyn ProgressSink>) -> Self {
        let repos = context
            .repos
            .iter()
            .filter(|repo| repo.is_writable())
            .filter_map(|repo| {
                Some(HermeticCheckpointRepo {
                    dir: repo.dir.clone(),
                    branch: repo.branch_hint.clone()?,
                })
            })
            .collect();
        Self {
            cwd: cwd.to_path_buf(),
            repos,
            correlation_key: context.correlation_key.clone(),
            progress,
            step: AtomicU32::new(2),
        }
    }

    fn next_step(&self) -> u32 {
        self.step.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl CheckpointHook for HermeticCheckpointer {
    async fn checkpoint(&self, label: &str) -> Result<Option<String>, String> {
        let step = self.step.fetch_add(1, Ordering::SeqCst);
        let cwd = self.cwd.clone();
        let repos = self.repos.clone();
        let label = label.to_string();
        let label_for_job = label.clone();
        let outcome = skein::runtime::spawn_blocking(move || {
            checkpoint_sync(&cwd, &repos, step, &label_for_job)
        })
        .await;
        match outcome {
            Ok(Some(sha)) => {
                self.progress.report(StepProgress {
                    correlation_key: self.correlation_key.clone(),
                    step,
                    status: label,
                    state: StepState::Done,
                    pushed_sha: Some(sha.clone()),
                    note: None,
                });
                Ok(Some(sha))
            }
            Ok(None) => {
                let _ =
                    self.step
                        .compare_exchange(step + 1, step, Ordering::SeqCst, Ordering::SeqCst);
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }
}

fn checkpoint_sync(
    cwd: &Path,
    repos: &[HermeticCheckpointRepo],
    step: u32,
    label: &str,
) -> Result<Option<String>, String> {
    let mut pushed = None;
    for repo in repos {
        git_in(cwd, &repo.dir, &["add", "-A"])?;
        if git_in(cwd, &repo.dir, &["diff", "--cached", "--quiet"]).is_ok() {
            continue;
        }
        git_in(
            cwd,
            &repo.dir,
            &["commit", "-m", &format!("checkpoint(step {step}): {label}")],
        )?;
        let push_ref = format!("HEAD:refs/heads/{}", repo.branch);
        git_in(cwd, &repo.dir, &["push", "origin", &push_ref])?;
        let sha = git_in(cwd, &repo.dir, &["rev-parse", "HEAD"])?
            .trim()
            .to_string();
        if pushed.is_none() {
            pushed = Some(sha);
        }
    }
    Ok(pushed)
}

fn git_in(cwd: &Path, dir: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd.join(dir))
        .output()
        .map_err(|error| format!("spawn git: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed ({}): {}",
            args.first().copied().unwrap_or(""),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout).map_err(|error| format!("git output not UTF-8: {error}"))
}

pub(crate) struct DaemonProgressSink {
    handle: RuntimeHandle,
    daemon: Arc<Daemon>,
    worker_id: String,
}

impl DaemonProgressSink {
    pub(crate) fn new(handle: RuntimeHandle, daemon: Arc<Daemon>, worker_id: String) -> Self {
        Self {
            handle,
            daemon,
            worker_id,
        }
    }
}

impl ProgressSink for DaemonProgressSink {
    fn report(&self, progress: StepProgress) {
        let message = temper_worker::progress_message(&self.worker_id, &progress);
        let daemon = self.daemon.clone();
        self.handle.spawn_with_cx(move |_cx| async move {
            let _ = daemon.deliver_protocol_message(message).await;
        });
    }
}

pub(crate) struct DaemonPrFreshnessGuard {
    daemon: Arc<Daemon>,
}

impl DaemonPrFreshnessGuard {
    pub(crate) fn new(daemon: Arc<Daemon>) -> Self {
        Self { daemon }
    }
}

impl PrFreshnessGuard for DaemonPrFreshnessGuard {
    fn check<'a>(
        &'a self,
        check: &'a PullRequestFreshness,
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

fn agent_error(error: CodingAgentError) -> AgentRunError {
    match error {
        CodingAgentError::NoProduct | CodingAgentError::UndeclaredVerdict { .. } => {
            AgentRunError::permanent(error.to_string())
        }
        CodingAgentError::Provider(_)
        | CodingAgentError::Run(_)
        | CodingAgentError::AgentStopped(_)
        | CodingAgentError::ModelUnavailable { .. }
        | CodingAgentError::Parse { .. } => AgentRunError::transient(error.to_string()),
    }
}
