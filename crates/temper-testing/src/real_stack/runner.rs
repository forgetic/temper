use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use skein::runtime::RuntimeHandle;
use temper_agent::{
    AgentAbortAuthority, CodingAgentError, ForgeContextHost, ProviderConfig, WorkspaceContext,
    run_coding_agent_native_with_totals_tool_config_and_hosts,
};
use temper_engine::Daemon;
use temper_protocol_agent::{AgentSessionState, PullRequestFreshness};
use temper_worker::{
    AcceptedSubmitProofStore, AgentForgeContextHost, AgentRunError, AgentRunOutput,
    AgentRunRequest, AgentRunner, AttemptFence, JobCancellation, PrFreshnessFailure,
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
    pub(crate) forge_context: AgentForgeContextHost,
    pub(crate) hooks: PauseHooks,
    pub(crate) observed_agent_sessions: Arc<Mutex<Vec<Option<AgentSessionState>>>>,
}

impl AgentRunner for NativeJigAgentRunner {
    async fn run(
        &self,
        job_id: &str,
        context: &WorkspaceContext,
        cwd: &Path,
    ) -> Result<AgentRunOutput, AgentRunError> {
        self.run_attempt(job_id, job_id, context, cwd, None).await
    }

    async fn run_request(
        &self,
        request: AgentRunRequest<'_>,
    ) -> Result<AgentRunOutput, AgentRunError> {
        self.run_attempt(
            request.job_id,
            &request.attempt_id,
            request.context,
            request.cwd,
            Some((request.fence, request.cancellation)),
        )
        .await
    }
}

impl NativeJigAgentRunner {
    async fn run_attempt(
        &self,
        job_id: &str,
        attempt_id: &str,
        context: &WorkspaceContext,
        cwd: &Path,
        attempt_control: Option<(AttemptFence, JobCancellation)>,
    ) -> Result<AgentRunOutput, AgentRunError> {
        self.observed_agent_sessions
            .lock()
            .expect("observed agent sessions lock")
            .push(context.agent_session.clone());
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
                let accepted_submit = accepted_submit_for_host.clone();
                let submit_for_pr = submit_for_pr.clone();
                Box::pin(async move {
                    temper_worker::handle_submit_for_pr_with_proof(
                        &accepted_submit,
                        move |request, context, cwd| submit_for_pr(request, context, cwd),
                        request,
                        context,
                        cwd,
                    )
                    .await
                })
            });
        let forge_host = self.forge_context.clone();
        let job_id = job_id.to_string();
        let attempt_id = attempt_id.to_string();
        let forge_context: ForgeContextHost =
            Arc::new(move |operation| forge_host(job_id.clone(), attempt_id.clone(), operation));
        let agent_cancellation = temper_agent::AgentCancellationLatch::default();
        let worker_cancellation = attempt_control
            .as_ref()
            .map(|(_, cancellation)| cancellation.clone());
        let _cancellation_owner = worker_cancellation
            .as_ref()
            .map(JobCancellation::register_async_owner);
        let run = run_coding_agent_native_with_totals_tool_config_and_hosts(
            self.handle.clone(),
            &self.provider,
            context,
            cwd,
            self.max_iterations,
            self.config_dir.as_deref(),
            self.enable_subagents,
            None,
            Some(submit_for_pr),
            Some(forge_context),
            temper_agent::AgentActivityConfig {
                cancellation: agent_cancellation.clone(),
                ..Default::default()
            },
            temper_protocol_agent::AgentRuntimeLimitsV1::default(),
        );
        let outcome = if let Some(worker_cancellation) = worker_cancellation {
            let mut run = std::pin::pin!(run);
            let mut cancelled = std::pin::pin!(worker_cancellation.cancelled());
            let mut forwarded = false;
            std::future::poll_fn(|cx| {
                if !forwarded && cancelled.as_mut().poll(cx).is_ready() {
                    forwarded = true;
                    agent_cancellation.request_cancel();
                }
                run.as_mut().poll(cx)
            })
            .await
        } else {
            run.await
        };
        let (result, _totals) = outcome.map_err(|error| {
            let worker_cancellation_requested =
                attempt_control
                    .as_ref()
                    .is_some_and(|(fence, cancellation)| {
                        cancellation.is_cancelled() || !fence.is_open()
                    });
            agent_error(error, worker_cancellation_requested)
        })?;
        Ok(AgentRunOutput {
            result,
            accepted_submit: accepted_submit.latest(),
        })
    }

    pub(crate) fn observed_agent_sessions(&self) -> Vec<Option<AgentSessionState>> {
        self.observed_agent_sessions
            .lock()
            .expect("observed agent sessions lock")
            .clone()
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

fn agent_error(error: CodingAgentError, worker_cancellation_requested: bool) -> AgentRunError {
    let class = match &error {
        CodingAgentError::Aborted { authority } => {
            if *authority == AgentAbortAuthority::WorkerRequested || worker_cancellation_requested {
                temper_protocol_worker::FailureClass::Canceled
            } else {
                temper_protocol_worker::FailureClass::Transient
            }
        }
        CodingAgentError::NoProduct
        | CodingAgentError::UndeclaredVerdict { .. }
        | CodingAgentError::InvalidVerdictResult(_) => {
            temper_protocol_worker::FailureClass::Permanent
        }
        CodingAgentError::Provider(_)
        | CodingAgentError::Run(_)
        | CodingAgentError::ModelFailure(_)
        | CodingAgentError::AgentStopped(_)
        | CodingAgentError::BudgetExhausted { .. }
        | CodingAgentError::ModelUnavailable { .. }
        | CodingAgentError::CodebaseMemory(_)
        | CodingAgentError::Parse { .. } => temper_protocol_worker::FailureClass::Transient,
    };
    AgentRunError::new(class, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use temper_protocol_worker::FailureClass;

    #[test]
    fn native_runner_classifies_typed_stop_errors() {
        let budget = agent_error(
            CodingAgentError::BudgetExhausted { max_iterations: 7 },
            false,
        );
        assert_eq!(budget.class, FailureClass::Transient);
        assert!(budget.message.contains("budget_exhausted"));

        let requested = agent_error(
            CodingAgentError::Aborted {
                authority: AgentAbortAuthority::WorkerRequested,
            },
            false,
        );
        assert_eq!(requested.class, FailureClass::Canceled);

        let unrequested = agent_error(
            CodingAgentError::Aborted {
                authority: AgentAbortAuthority::Unrequested,
            },
            false,
        );
        assert_eq!(unrequested.class, FailureClass::Transient);

        let fenced = agent_error(
            CodingAgentError::Aborted {
                authority: AgentAbortAuthority::Unrequested,
            },
            true,
        );
        assert_eq!(fenced.class, FailureClass::Canceled);
    }

    #[test]
    fn native_runner_keeps_model_failures_transient() {
        let failure = agent_error(
            CodingAgentError::ModelFailure(Box::new(
                temper_agent_core::ModelFailureDiagnostic::redacted_unknown(
                    "provider", "model", true,
                ),
            )),
            false,
        );

        assert_eq!(failure.class, FailureClass::Transient);
        assert!(failure.message.contains("redacted_unknown"));
        assert!(failure.message.contains("retryable=true"));
    }

    #[test]
    fn native_runner_retains_parse_and_permanent_classifications() {
        let parse = agent_error(
            CodingAgentError::Parse {
                snippet: "not json".to_string(),
                error: "expected value".to_string(),
            },
            false,
        );
        assert_eq!(parse.class, FailureClass::Transient);
        assert_eq!(
            agent_error(CodingAgentError::NoProduct, false).class,
            FailureClass::Permanent
        );
    }
}
