use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use skein::runtime::RuntimeHandle;
use temper_agent::{
    CodingAgentError, ProviderConfig, WorkspaceContext,
    run_coding_agent_native_with_options_and_submit_for_pr,
};
use temper_engine::Daemon;
use temper_protocol_agent::PullRequestFreshness;
use temper_worker::{
    AcceptedSubmitProofStore, AgentRunError, AgentRunOutput, AgentRunner, PrFreshnessFailure,
    PrFreshnessGuard,
};

use super::pause::{PauseHooks, PausePoint};

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
    pub(crate) submit_for_pr: temper_agent::SubmitForPrHost,
    pub(crate) hooks: PauseHooks,
}

impl AgentRunner for NativeJigAgentRunner {
    async fn run(
        &self,
        context: &WorkspaceContext,
        cwd: &Path,
    ) -> Result<AgentRunOutput, AgentRunError> {
        // CodingExecutor invokes the runner only after checkout recovery and
        // durable agent-session attachment. This is therefore the stable seam
        // for restart tests that must mutate or inspect a prepared workspace
        // without racing the model or relying on a sleep.
        self.hooks.reach(PausePoint::AgentSessionStarted).await;
        let accepted_submit = AcceptedSubmitProofStore::new();
        let submit_for_pr = self.submit_for_pr.clone();
        let accepted_submit_for_host = accepted_submit.clone();
        let submit_for_pr: temper_agent::SubmitForPrHost =
            std::sync::Arc::new(move |request, context, cwd| {
                temper_worker::handle_submit_for_pr_with_proof(
                    &accepted_submit_for_host,
                    |request, context, cwd| submit_for_pr(request, context, cwd),
                    request,
                    context,
                    cwd,
                )
            });
        let result = run_coding_agent_native_with_options_and_submit_for_pr(
            self.handle.clone(),
            &self.provider,
            context,
            cwd,
            self.max_iterations,
            self.config_dir.as_deref(),
            self.enable_subagents,
            Some(submit_for_pr),
        )
        .await
        .map_err(agent_error)?;
        Ok(AgentRunOutput {
            result,
            accepted_submit: accepted_submit.latest(),
        })
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
        CodingAgentError::NoProduct
        | CodingAgentError::UndeclaredVerdict { .. }
        | CodingAgentError::InvalidVerdictResult(_) => AgentRunError::permanent(error.to_string()),
        CodingAgentError::Provider(_)
        | CodingAgentError::Run(_)
        | CodingAgentError::AgentStopped(_)
        | CodingAgentError::ModelUnavailable { .. }
        | CodingAgentError::CodebaseMemory(_)
        | CodingAgentError::Parse { .. } => AgentRunError::transient(error.to_string()),
    }
}
