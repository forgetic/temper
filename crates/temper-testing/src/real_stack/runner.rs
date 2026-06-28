use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use skein::runtime::RuntimeHandle;
use temper_agent::{
    CodingAgentError, ProviderConfig, WorkspaceContext, WorkspaceResult,
    run_coding_agent_native_with_options,
};
use temper_engine::Daemon;
use temper_protocol_agent::{PullRequestFreshness, StepProgress};
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
}

impl AgentRunner for NativeJigAgentRunner {
    async fn run(
        &self,
        context: &WorkspaceContext,
        cwd: &Path,
        _progress: Arc<dyn ProgressSink>,
    ) -> Result<WorkspaceResult, AgentRunError> {
        run_coding_agent_native_with_options(
            self.handle.clone(),
            &self.provider,
            context,
            cwd,
            self.max_iterations,
            self.config_dir.as_deref(),
            self.enable_subagents,
        )
        .await
        .map_err(agent_error)
    }
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
