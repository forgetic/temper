use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use skein::runtime::RuntimeHandle;
use temper_agent::{
    AgentAbortAuthority, CodingAgentError, ForgeContextHost, ProviderConfig, WorkspaceContext,
    run_coding_agent_native_with_totals_tool_config_and_hosts,
};
use temper_engine::Daemon;
use temper_protocol_activity::FailureCodeV1;
use temper_protocol_agent::{AgentLifecycleEventV1, AgentSessionState, PullRequestFreshness};
use temper_worker::{
    AcceptedSubmitProofStore, AgentForgeContextHost, AgentRunError, AgentRunOutput,
    AgentRunRequest, AgentRunner, AttemptFence, JobCancellation, JobProgressReporter,
    PrFreshnessFailure, PrFreshnessGuard, TraceCollector,
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
    pub(crate) trace_policy: temper_protocol_activity::AgentActivityCapturePolicyV1,
    pub(crate) trace_collector: TraceCollector,
    pub(crate) activity: Arc<HermeticActivityCounters>,
    pub(crate) observed_agent_sessions: Arc<Mutex<Vec<Option<AgentSessionState>>>>,
}

/// Content-free operation counts observed at the native-agent boundary.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HermeticActivitySnapshot {
    pub model_started: usize,
    pub model_finished: usize,
    pub tool_started: usize,
    pub tool_finished: usize,
    pub forge_context_started: usize,
    pub forge_context_finished: usize,
    pub submit_started: usize,
    pub submit_finished: usize,
}

#[derive(Default)]
pub(crate) struct HermeticActivityCounters {
    model_started: AtomicUsize,
    model_finished: AtomicUsize,
    tool_started: AtomicUsize,
    tool_finished: AtomicUsize,
    forge_context_started: AtomicUsize,
    forge_context_finished: AtomicUsize,
    submit_started: AtomicUsize,
    submit_finished: AtomicUsize,
}

impl HermeticActivityCounters {
    pub(crate) fn forge_context_started(&self) {
        self.forge_context_started.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn forge_context_finished(&self) {
        self.forge_context_finished.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn submit_started(&self) {
        self.submit_started.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn submit_finished(&self) {
        self.submit_finished.fetch_add(1, Ordering::SeqCst);
    }

    fn record_lifecycle(&self, event: &AgentLifecycleEventV1) {
        let counter = match event {
            AgentLifecycleEventV1::ModelStarted { .. } => &self.model_started,
            AgentLifecycleEventV1::ModelFinished { .. } => &self.model_finished,
            AgentLifecycleEventV1::ToolStarted { .. } => &self.tool_started,
            AgentLifecycleEventV1::ToolFinished { .. } => &self.tool_finished,
            AgentLifecycleEventV1::ModelProgress { .. }
            | AgentLifecycleEventV1::ModelRetrying { .. }
            | AgentLifecycleEventV1::AgentFinished { .. }
            | AgentLifecycleEventV1::Containment { .. }
            | AgentLifecycleEventV1::SteeringApplied => return,
        };
        counter.fetch_add(1, Ordering::SeqCst);
    }

    fn snapshot(&self) -> HermeticActivitySnapshot {
        HermeticActivitySnapshot {
            model_started: self.model_started.load(Ordering::SeqCst),
            model_finished: self.model_finished.load(Ordering::SeqCst),
            tool_started: self.tool_started.load(Ordering::SeqCst),
            tool_finished: self.tool_finished.load(Ordering::SeqCst),
            forge_context_started: self.forge_context_started.load(Ordering::SeqCst),
            forge_context_finished: self.forge_context_finished.load(Ordering::SeqCst),
            submit_started: self.submit_started.load(Ordering::SeqCst),
            submit_finished: self.submit_finished.load(Ordering::SeqCst),
        }
    }
}

impl AgentRunner for NativeJigAgentRunner {
    async fn run(
        &self,
        job_id: &str,
        context: &WorkspaceContext,
        cwd: &Path,
    ) -> Result<AgentRunOutput, AgentRunError> {
        self.run_attempt(
            job_id,
            job_id,
            context,
            cwd,
            AttemptFence::open(),
            JobCancellation::default(),
            JobProgressReporter::noop(job_id),
        )
        .await
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
            request.fence,
            request.cancellation,
            request.progress,
        )
        .await
    }
}

impl NativeJigAgentRunner {
    #[allow(clippy::too_many_arguments)]
    async fn run_attempt(
        &self,
        job_id: &str,
        attempt_id: &str,
        context: &WorkspaceContext,
        cwd: &Path,
        fence: AttemptFence,
        cancellation: JobCancellation,
        progress: JobProgressReporter,
    ) -> Result<AgentRunOutput, AgentRunError> {
        self.observed_agent_sessions
            .lock()
            .expect("observed agent sessions lock")
            .push(context.agent_session.clone());
        let tracing_required = self.trace_collector.tracing_enabled();
        let trace = self
            .trace_collector
            .begin_run(job_id, context)
            .ok()
            .flatten();
        let activity_endpoint = trace.as_ref().and_then(|trace| trace.bind_endpoint().ok());
        let activity_address = activity_endpoint
            .as_ref()
            .map(|endpoint| endpoint.address().to_string());
        // Register cancellation ownership before the pause. Ownership-loss
        // acceptance parks here and requires the runner to persist and forward
        // its cancelled trace boundary before reporting quiescence.
        let _cancellation_owner = cancellation.register_async_owner();
        // CodingExecutor invokes the runner only after checkout recovery and
        // durable agent-session attachment. This is therefore the stable seam
        // for restart tests that must mutate or inspect a prepared workspace
        // without racing the model or relying on a sleep. Component crash may
        // cancel while a test holds this pause, so cancellation also releases
        // the runner's async owner without requiring the test permit.
        let mut session_pause = std::pin::pin!(self.hooks.reach(PausePoint::AgentSessionStarted));
        let mut pause_cancellation = std::pin::pin!(cancellation.cancelled());
        std::future::poll_fn(|cx| {
            if pause_cancellation.as_mut().poll(cx).is_ready()
                || session_pause.as_mut().poll(cx).is_ready()
            {
                std::task::Poll::Ready(())
            } else {
                std::task::Poll::Pending
            }
        })
        .await;
        let accepted_submit = AcceptedSubmitProofStore::new();
        let submit_for_pr = self.submit_for_pr.clone();
        let accepted_submit_for_host = accepted_submit.clone();
        let submit_fence = fence.clone();
        let submit_cancellation = cancellation.clone();
        let submit_for_pr: temper_agent::SubmitForPrHost =
            std::sync::Arc::new(move |request, context, cwd| {
                let accepted_submit = accepted_submit_for_host.clone();
                let submit_for_pr = submit_for_pr.clone();
                let fence = submit_fence.clone();
                let cancellation = submit_cancellation.clone();
                Box::pin(async move {
                    temper_worker::handle_submit_for_pr_with_proof(
                        &accepted_submit,
                        &fence,
                        &cancellation,
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
        let forge_fence = fence.clone();
        let forge_context: ForgeContextHost = Arc::new(move |operation| {
            let forge_host = forge_host.clone();
            let job_id = job_id.clone();
            let attempt_id = attempt_id.clone();
            let fence = forge_fence.clone();
            Box::pin(async move {
                if !fence.is_open() {
                    return Err(temper_protocol_worker::ForgeContextErrorCode::ForgeUnavailable);
                }
                let result = forge_host(job_id, attempt_id, fence.clone(), operation).await;
                if fence.is_open() {
                    result
                } else {
                    Err(temper_protocol_worker::ForgeContextErrorCode::ForgeUnavailable)
                }
            })
        });
        let agent_cancellation = temper_agent::AgentCancellationLatch::default();
        let activity = Arc::clone(&self.activity);
        let lifecycle_reporter: temper_agent::AgentLifecycleReporter =
            Arc::new(move |scope, event| {
                activity.record_lifecycle(&event);
                let _ = progress.report(scope, event);
            });
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
                policy: self.trace_policy.clone(),
                address: activity_address,
                lifecycle_address: None,
                lifecycle_reporter: Some(lifecycle_reporter),
                cancellation: agent_cancellation.clone(),
            },
            temper_protocol_agent::AgentRuntimeLimitsV1::default(),
        );
        let outcome = if !fence.is_open() || cancellation.is_cancelled() {
            Err(CodingAgentError::Aborted {
                authority: AgentAbortAuthority::WorkerRequested,
            })
        } else {
            let mut run = std::pin::pin!(run);
            let mut observed = None;
            std::future::poll_fn(|cx| {
                while let std::task::Poll::Ready(request) = cancellation.poll_request(observed, cx)
                {
                    observed = Some(request);
                    let stage = match request {
                        temper_worker::JobCancellationRequest::Graceful => {
                            temper_protocol_agent::AgentCancellationStage::Graceful
                        }
                        temper_worker::JobCancellationRequest::ForcedTermination => {
                            temper_protocol_agent::AgentCancellationStage::ForcedTermination
                        }
                        temper_worker::JobCancellationRequest::HardKill => {
                            temper_protocol_agent::AgentCancellationStage::HardKill
                        }
                    };
                    agent_cancellation.request(stage);
                }
                run.as_mut().poll(cx)
            })
            .await
        };
        let worker_cancellation_requested = cancellation.is_cancelled() || !fence.is_open();
        let outcome = match outcome {
            Ok(_) if worker_cancellation_requested => Err(CodingAgentError::Aborted {
                authority: AgentAbortAuthority::WorkerRequested,
            }),
            outcome => outcome,
        };
        if worker_cancellation_requested {
            accepted_submit.clear();
        }
        if let Some(endpoint) = activity_endpoint {
            endpoint.stop();
        }
        if let Some(trace) = trace {
            let terminal = if worker_cancellation_requested {
                trace.finish_cancelled()
            } else {
                match &outcome {
                    Ok(_) => trace.finish_success(None),
                    Err(error) => trace
                        .finish_failure(FailureCodeV1::Internal, agent_error_class(error, false)),
                }
            };
            match terminal {
                Ok(sequence) if worker_cancellation_requested => {
                    cancellation.quiescence_pending(format!(
                        "terminal trace {} sequence {sequence} is awaiting durable acknowledgement",
                        trace.run_id()
                    ));
                    self.trace_collector
                        .await_terminal_acknowledged(trace.run_id(), sequence)
                        .await;
                }
                Ok(sequence) => {
                    let _ = self
                        .trace_collector
                        .await_acknowledged(trace.run_id(), sequence, Duration::from_secs(1))
                        .await;
                }
                Err(error) if worker_cancellation_requested => {
                    cancellation.quiescence_pending(format!(
                        "cancelled terminal trace {} could not be persisted: {error}",
                        trace.run_id()
                    ));
                    std::future::pending::<()>().await;
                }
                Err(_) => {}
            }
        } else if tracing_required && worker_cancellation_requested {
            cancellation
                .quiescence_pending("enabled durable tracing did not create a cancellation run");
            std::future::pending::<()>().await;
        }
        let (result, _totals) =
            outcome.map_err(|error| agent_error(error, worker_cancellation_requested))?;
        if !fence.is_open() || cancellation.is_cancelled() {
            accepted_submit.clear();
            return Err(AgentRunError::new(
                temper_protocol_worker::FailureClass::Canceled,
                "agent attempt is no longer available",
            ));
        }
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

    pub(crate) fn activity_snapshot(&self) -> HermeticActivitySnapshot {
        self.activity.snapshot()
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
    let class = agent_error_class(&error, worker_cancellation_requested);
    AgentRunError::new(class, error.to_string())
}

fn agent_error_class(
    error: &CodingAgentError,
    worker_cancellation_requested: bool,
) -> temper_protocol_worker::FailureClass {
    match error {
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
    }
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
