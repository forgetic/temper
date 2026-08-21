// SPDX-License-Identifier: MPL-2.0

//! The standalone in-process coding-agent runner.
//!
//! [`InProcessAgentRunner`] implements the orchestrator's
//! [`AgentRunner`](temper_worker::AgentRunner) by calling the agent core
//! ([`run_coding_agent_native_with_totals`]) directly on the host event loop —
//! no subprocess, no temp files. `WorkspaceContext` flows in as a value and
//! `WorkspaceResult` comes back as the return value.
//!
//! This is the worker→agent carrier the standalone daemon uses; the distributed
//! deployment keeps the subprocess `OutOfProcessRunner`.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Instant;

use skein::runtime::RuntimeHandle;
use temper_agent::{
    AgentAbortAuthority, AgentActivityConfig, AgentCancellationLatch, AgentContainmentContext,
    CodingAgentError, ProviderConfig, RunTotals, SubmitForPrHost, protocol_model_failure,
    run_coding_agent_native_with_totals_tool_config_hosts_and_containment,
};
use temper_config::AgentActivityCapturePolicyV1;
use temper_log::WorkItemRef;
use temper_log::emit::{
    AgentFinished, AgentStarted, AgentTerminalReasonV1, AgentTerminalStatus, emit_agent_finished,
    emit_agent_started,
};
use temper_protocol_activity::ModelFailureV1;
use temper_protocol_agent::{
    AgentCancellationStage, AgentRuntimeLimitsV1, AgentToolConfig, WorkspaceContext,
};
use temper_protocol_worker::FailureClass;
use temper_worker::{
    AcceptedSubmitProofStore, AgentForgeContextHost, AgentRunError, AgentRunOutput,
    AgentRunRequest, AgentRunner, JobCancellationRequest, TraceCollector, WorkerAgentTraceConfig,
};

mod attempt_fencing;
mod terminal;

/// Runs coding/triage/review turns in-process on the host loop.
pub struct InProcessAgentRunner {
    handle: RuntimeHandle,
    provider: ProviderConfig,
    max_iterations: usize,
    config_dir: Option<PathBuf>,
    enable_subagents: bool,
    tool_config: Option<AgentToolConfig>,
    runtime_limits: AgentRuntimeLimitsV1,
    trace_policy: AgentActivityCapturePolicyV1,
    trace_collector: TraceCollector,
    submit_for_pr: SubmitForPrHost,
    forge_context: Option<AgentForgeContextHost>,
    containment: AgentContainmentContext,
}

impl InProcessAgentRunner {
    pub fn new(
        handle: RuntimeHandle,
        provider: ProviderConfig,
        max_iterations: usize,
        config_dir: Option<PathBuf>,
        enable_subagents: bool,
    ) -> Self {
        Self {
            handle,
            provider,
            max_iterations,
            config_dir,
            enable_subagents,
            tool_config: None,
            runtime_limits: AgentRuntimeLimitsV1::default(),
            trace_policy: AgentActivityCapturePolicyV1::default(),
            trace_collector: TraceCollector::default(),
            submit_for_pr: std::sync::Arc::new(|request, context, cwd| {
                Box::pin(async move {
                    temper_worker::submit_for_pr_pre_push_response(&request, &context, cwd).await
                })
            }),
            forge_context: None,
            containment: AgentContainmentContext::production(None),
        }
    }

    /// Replaces the standalone process-containment factory. This is instance
    /// scoped so tests can force a backend without ambient environment state.
    #[must_use]
    pub fn with_containment_context(mut self, containment: AgentContainmentContext) -> Self {
        self.containment = containment;
        self
    }

    /// Sets the non-secret agent tool config stored with this in-process
    /// runner. The native coding loop registers codebase-memory tools from it
    /// when it applies to the current role.
    #[must_use]
    pub fn with_tool_config(mut self, tool_config: Option<AgentToolConfig>) -> Self {
        self.tool_config = tool_config;
        self
    }

    /// Stores complete first-party operation limits for the native loop and
    /// every nested subagent.
    #[must_use]
    pub fn with_runtime_limits(mut self, runtime_limits: AgentRuntimeLimitsV1) -> Self {
        self.runtime_limits = runtime_limits;
        self
    }

    /// Stores the same effective capture policy used by split-mode agents.
    #[must_use]
    pub fn with_trace_policy(mut self, trace_policy: AgentActivityCapturePolicyV1) -> Self {
        self.trace_policy = trace_policy;
        self
    }

    /// Configures the same worker-owned collector used by split-mode runs.
    ///
    /// This compatibility builder creates an independent collector. Product
    /// composition roots should prefer [`Self::with_shared_trace_collector`].
    #[must_use]
    pub fn with_trace_collector(self, config: WorkerAgentTraceConfig) -> Self {
        self.with_shared_trace_collector(TraceCollector::new(config))
    }

    /// Uses a clone-shared worker-owned collector for this producer.
    #[must_use]
    pub fn with_shared_trace_collector(mut self, collector: TraceCollector) -> Self {
        self.trace_collector = collector;
        self
    }

    pub fn trace_policy(&self) -> &AgentActivityCapturePolicyV1 {
        &self.trace_policy
    }

    /// Returns the stored tool config when it applies to `role`.
    pub fn tool_config_for_role(&self, role: &str) -> Option<&AgentToolConfig> {
        self.tool_config
            .as_ref()
            .filter(|config| config.enabled_for_role(role))
    }

    /// Overrides the host-controlled `submit_for_pr` gate used by writable
    /// engineer sessions.
    #[must_use]
    pub fn with_submit_for_pr_host(mut self, submit_for_pr: SubmitForPrHost) -> Self {
        self.submit_for_pr = submit_for_pr;
        self
    }

    /// Installs an asynchronous assignment-bound Forge context host.
    #[must_use]
    pub fn with_forge_context_host(mut self, forge_context: AgentForgeContextHost) -> Self {
        self.forge_context = Some(forge_context);
        self
    }
}

impl AgentRunner for InProcessAgentRunner {
    fn run(
        &self,
        job_id: &str,
        context: &WorkspaceContext,
        cwd: &Path,
    ) -> impl std::future::Future<Output = Result<AgentRunOutput, AgentRunError>> + Send {
        self.run_attempt(AgentRunRequest::unsupervised(job_id, context, cwd))
    }

    fn run_request(
        &self,
        request: AgentRunRequest<'_>,
    ) -> impl std::future::Future<Output = Result<AgentRunOutput, AgentRunError>> + Send {
        self.run_attempt(request)
    }
}

impl InProcessAgentRunner {
    fn run_attempt(
        &self,
        request: AgentRunRequest<'_>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<AgentRunOutput, AgentRunError>>
                + Send
                + 'static,
        >,
    > {
        let job_id = request.job_id;
        let attempt_id = request.attempt_id;
        let context = request.context;
        let cwd = request.cwd;
        let fence = request.fence;
        let cancellation = request.cancellation;
        let emergency_termination = request.emergency_termination;
        let progress = request.progress;
        // §7 agent boundary events. The `item` ref is the work-item subject tag
        // (`[repo#n]` / `[repo PR#n]`); `kind` is the role's activity verb
        // (architect→triage, engineer→coding). We emit `agent.started` here,
        // up-front and synchronously — *before* the async block / model call —
        // so the start line appears even if the run later stalls on the model.
        let role = context.work_item.role.clone();
        let item = work_item_ref(context);
        let kind = run_kind(&role);
        let started = Instant::now();
        let tracing_required = self.trace_collector.tracing_enabled();
        let trace = match self.trace_collector.begin_run(job_id, context) {
            Ok(trace) => trace,
            Err(error) => {
                temper_worker::warn_activity_trace_start_failed(
                    temper_worker::ActivityTraceRunner::Standalone,
                    job_id,
                    context.correlation_key.as_str(),
                    &error,
                );
                None
            }
        };
        let activity_endpoint = trace.as_ref().and_then(|trace| match trace.bind_endpoint() {
            Ok(endpoint) => Some(endpoint),
            Err(error) => {
                tracing::warn!(
                    target: "temper::worker",
                    service = "worker",
                    event = "agent.activity.endpoint_failed",
                    job_id,
                    run_id = trace.run_id(),
                    %error,
                    "standalone worker could not bind agent activity transport; continuing without it"
                );
                None
            }
        });
        let activity_address = activity_endpoint
            .as_ref()
            .map(|endpoint| endpoint.address().to_string());
        if let Some(item) = item.as_ref() {
            emit_agent_started(AgentStarted {
                item,
                role: &role,
                kind,
                detail: started_detail(kind),
            });
        }

        let handle = self.handle.clone();
        let provider = self.provider.clone();
        let max_iterations = self.max_iterations;
        let config_dir = self.config_dir.clone();
        let enable_subagents = self.enable_subagents;
        let tool_config = self.tool_config.clone();
        let runtime_limits = self.runtime_limits;
        let trace_policy = self.trace_policy.clone();
        let trace_collector = self.trace_collector.clone();
        let accepted_submit = AcceptedSubmitProofStore::new();
        let submit_for_pr = attempt_fencing::submit_host(
            accepted_submit.clone(),
            self.submit_for_pr.clone(),
            fence.clone(),
            cancellation.clone(),
        );
        let submit_for_pr = Some(submit_for_pr);
        let forge_context = self.forge_context.clone().map(|host| {
            attempt_fencing::forge_host(host, job_id.to_string(), attempt_id.clone(), fence.clone())
        });
        let containment = self
            .containment
            .clone()
            .with_emergency_registry(emergency_termination);
        let agent_cancellation = AgentCancellationLatch::default();
        let cancellation_owner = cancellation.register_async_owner();
        let lifecycle_reporter: temper_agent::AgentLifecycleReporter =
            std::sync::Arc::new(move |scope, event| {
                let _ = progress.report(scope, event);
            });
        let context = context.clone();
        let cwd = cwd.to_path_buf();

        Box::pin(async move {
            let _cancellation_owner = cancellation_owner;
            let mut outcome = if !fence.is_open() || cancellation.is_cancelled() {
                Err(CodingAgentError::Aborted {
                    authority: AgentAbortAuthority::WorkerRequested,
                })
            } else {
                let run = run_coding_agent_native_with_totals_tool_config_hosts_and_containment(
                    handle,
                    &provider,
                    &context,
                    &cwd,
                    max_iterations,
                    config_dir.as_deref(),
                    enable_subagents,
                    tool_config.as_ref(),
                    submit_for_pr,
                    forge_context,
                    AgentActivityConfig {
                        policy: trace_policy,
                        address: activity_address,
                        lifecycle_address: None,
                        lifecycle_reporter: Some(lifecycle_reporter),
                        cancellation: agent_cancellation.clone(),
                        operator_transcript: None,
                    },
                    runtime_limits,
                    containment,
                );
                let mut run = std::pin::pin!(run);
                let mut observed_cancellation = None;
                std::future::poll_fn(|cx| {
                    while let std::task::Poll::Ready(stage) =
                        cancellation.poll_request(observed_cancellation, cx)
                    {
                        observed_cancellation = Some(stage);
                        agent_cancellation.request(agent_cancellation_stage(stage));
                    }
                    run.as_mut().poll(cx)
                })
                .await
            };
            // Managed bash/MCP owners clean up on dedicated threads. Keep the
            // attempt owner registered until each process boundary has produced
            // ordinary recursive-empty proof; emergency dispatch alone never
            // satisfies this join.
            cancellation.wait_for_process_owners().await;

            // A completion that races authoritative cancellation is never a
            // successful model/result completion at the worker boundary.
            let worker_cancellation_requested = cancellation.is_cancelled() || !fence.is_open();
            if worker_cancellation_requested && outcome.is_ok() {
                outcome = Err(CodingAgentError::Aborted {
                    authority: AgentAbortAuthority::WorkerRequested,
                });
            }
            if worker_cancellation_requested {
                accepted_submit.clear();
            }
            let (terminal_status, terminal_reason) =
                agent_terminal_report(&outcome, worker_cancellation_requested);

            if let Some(endpoint) = activity_endpoint {
                endpoint.stop();
            }
            terminal::finish_and_acknowledge(
                trace,
                &trace_collector,
                &cancellation,
                tracing_required,
                worker_cancellation_requested,
                &outcome,
            )
            .await;

            // §7 `agent.finished` on both paths. Typed status and terminal
            // reason keep abnormal agent stops queryable and ensure failures
            // and cancellations are never rendered as successful `done` lines.
            let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            if let Some(item) = item.as_ref() {
                let summary = match &outcome {
                    Ok((result, totals)) => {
                        let base = result.summary.clone().unwrap_or_else(|| "done".to_string());
                        format!("{base} | {}", totals_suffix(*totals))
                    }
                    Err(error) => error.to_string(),
                };
                let model_failure = outcome.as_ref().err().and_then(coding_agent_model_failure);
                emit_agent_finished(AgentFinished {
                    item,
                    role: &role,
                    kind,
                    status: terminal_status,
                    terminal_reason,
                    model_failure: model_failure.as_ref(),
                    duration_ms,
                    summary: &summary,
                });
            }

            // Conversion to the generic worker boundary happens only after the
            // trace and operator event have selected their typed terminal data.
            if !fence.is_open() || cancellation.is_cancelled() {
                accepted_submit.clear();
                return Err(AgentRunError::new(
                    FailureClass::Canceled,
                    "agent attempt is no longer available",
                ));
            }
            let (result, _totals) = outcome.map_err(|error| {
                classify_coding_agent_error(error, worker_cancellation_requested)
            })?;

            Ok(AgentRunOutput {
                result,
                accepted_submit: accepted_submit.latest(),
                operator_transcript: Vec::new(),
            })
        })
    }
}

fn agent_cancellation_stage(request: JobCancellationRequest) -> AgentCancellationStage {
    match request {
        JobCancellationRequest::Graceful => AgentCancellationStage::Graceful,
        JobCancellationRequest::ForcedTermination => AgentCancellationStage::ForcedTermination,
        JobCancellationRequest::HardKill => AgentCancellationStage::HardKill,
    }
}

fn coding_agent_model_failure(error: &CodingAgentError) -> Option<ModelFailureV1> {
    match error {
        CodingAgentError::ModelFailure(diagnostic)
        | CodingAgentError::ModelUnavailable { diagnostic, .. } => {
            Some(protocol_model_failure(diagnostic.as_ref()))
        }
        _ => None,
    }
}

fn agent_terminal_report<T>(
    outcome: &Result<T, CodingAgentError>,
    worker_cancellation_requested: bool,
) -> (AgentTerminalStatus, Option<AgentTerminalReasonV1>) {
    match outcome {
        Ok(_) => (
            AgentTerminalStatus::Succeeded,
            Some(AgentTerminalReasonV1::Completed),
        ),
        Err(
            CodingAgentError::AgentStopped(_)
            | CodingAgentError::ModelFailure(_)
            | CodingAgentError::ModelUnavailable { .. },
        ) => (
            AgentTerminalStatus::Failed,
            Some(AgentTerminalReasonV1::ModelError),
        ),
        Err(CodingAgentError::BudgetExhausted { .. }) => (
            AgentTerminalStatus::Failed,
            Some(AgentTerminalReasonV1::BudgetExhausted),
        ),
        Err(CodingAgentError::DecisionAnchorRecoveryExhausted) => (
            AgentTerminalStatus::Failed,
            Some(AgentTerminalReasonV1::DecisionAnchorRecoveryExhausted),
        ),
        Err(CodingAgentError::Aborted { authority }) => (
            if abort_is_authoritative(*authority, worker_cancellation_requested) {
                AgentTerminalStatus::Cancelled
            } else {
                AgentTerminalStatus::Failed
            },
            Some(AgentTerminalReasonV1::Aborted),
        ),
        Err(_) => (AgentTerminalStatus::Failed, None),
    }
}

fn abort_is_authoritative(
    authority: AgentAbortAuthority,
    worker_cancellation_requested: bool,
) -> bool {
    authority == AgentAbortAuthority::WorkerRequested || worker_cancellation_requested
}

/// The §7 run-kind (`role/kind`) for a worker role.
///
/// The human renderer formats `role/kind` (e.g. `architect/triage`,
/// `engineer/coding`). We map the role's *activity verb* here: architect
/// triages, the engineer codes, the reviewer reviews. Unknown roles fall back
/// to a neutral `run` so the line still reads sensibly.
fn run_kind(role: &str) -> &'static str {
    match role {
        "architect" => "triage",
        "engineer" => "coding",
        "reviewer" => "review",
        _ => "run",
    }
}

/// The one-line `detail` for the `agent.started` line, per §7's examples.
fn started_detail(kind: &str) -> &'static str {
    match kind {
        "triage" => "reading issue + repo context",
        "coding" => "preparing workspace, implementing",
        "review" => "reviewing changes",
        _ => "running",
    }
}

/// The token-totals suffix appended to a successful `agent.finished` summary.
///
/// Renders `<input> in / <output> out, <N> tool calls` with the token counts
/// humanized via [`human_count`] (the §7 example: `470k in / 6.4k out, 52 tool
/// calls`). The tool-call count is kept raw — it is a small cardinal number, not
/// a token volume — so a one-off run reads `1 tool call`.
fn totals_suffix(totals: RunTotals) -> String {
    let calls = totals.tool_calls;
    let unit = if calls == 1 {
        "tool call"
    } else {
        "tool calls"
    };
    format!(
        "{} in / {} out, {calls} {unit}",
        human_count(totals.input),
        human_count(totals.output),
    )
}

/// Humanizes a token count with a `k` suffix above 1000.
///
/// Under 1000 the raw integer is shown (`0`, `999`). At/above 1000 the value is
/// expressed in thousands: 1000–9999 keep one decimal of precision (`6379` ->
/// `6.4k`), 10_000 and up round to a whole `k` (`470306` -> `470k`). A trailing
/// `.0` is dropped so a round value stays tight (`2000` -> `2k`). Arithmetic is
/// integer-only (half-up rounding) to avoid float surprises.
fn human_count(n: u64) -> String {
    if n < 1000 {
        return n.to_string();
    }
    if n >= 10_000 {
        // Round to the nearest whole thousand: floor((n + 500) / 1000).
        let thousands = (n + 500) / 1000;
        return format!("{thousands}k");
    }
    // 1000–9999: one decimal, i.e. tenths = round(n / 100) half-up.
    let tenths = (n + 50) / 100;
    let whole = tenths / 10;
    let frac = tenths % 10;
    if frac == 0 {
        format!("{whole}k")
    } else {
        format!("{whole}.{frac}k")
    }
}

/// Builds the §7 work-item subject reference from the workspace context.
///
/// `repo` is the primary repository's bare `owner/name` path; the number and
/// issue-vs-PR kind come from the work item. Returns `None` when there is no
/// primary repo or the target number cannot be parsed — in that case the agent
/// events are skipped rather than logged against a bogus `#0` ref.
fn work_item_ref(context: &WorkspaceContext) -> Option<WorkItemRef> {
    let repo = context.primary()?;
    let repo_path = format!("{}/{}", repo.owner, repo.name);
    let number = parse_target_number(&context.work_item.target)?;
    let is_pr = context.work_item.kind == "pull_request";
    Some(if is_pr {
        WorkItemRef::pull_request(repo_path, number)
    } else {
        WorkItemRef::issue(repo_path, number)
    })
}

/// Extracts the artifact number from a Debug-formatted target string.
///
/// The worker builds `target` as e.g. `Issue { number: ItemNumber(7) }` or
/// `PullRequest { number: ItemNumber(44) }`
/// ([`temper_worker`]'s context assembly). For robustness this also accepts the
/// bare `number: 7` form some fixtures use. Returns `None` if no `number:`
/// segment with a parseable integer is found, so the caller can skip the emit.
fn parse_target_number(target: &str) -> Option<u64> {
    // Find the `number:` key, then take the first run of ASCII digits after it
    // (skipping the `ItemNumber(` wrapper if present).
    let after_key = target.split("number:").nth(1)?;
    let digits: String = after_key
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// Map an agent-core error to the worker's retry/cancellation classification.
fn classify_coding_agent_error(
    error: CodingAgentError,
    worker_cancellation_requested: bool,
) -> AgentRunError {
    let class = coding_agent_failure_class(&error, worker_cancellation_requested);
    let model_failure = coding_agent_model_failure(&error);
    let worker_error = AgentRunError::new(class, error.to_string());
    match model_failure {
        Some(model_failure) => worker_error.with_model_failure(model_failure),
        None => worker_error,
    }
}

fn coding_agent_failure_class(
    error: &CodingAgentError,
    worker_cancellation_requested: bool,
) -> FailureClass {
    match error {
        CodingAgentError::Aborted { authority } => {
            if abort_is_authoritative(*authority, worker_cancellation_requested) {
                FailureClass::Canceled
            } else {
                FailureClass::Transient
            }
        }
        CodingAgentError::Provider(_)
        | CodingAgentError::Run(_)
        | CodingAgentError::ModelFailure(_)
        | CodingAgentError::AgentStopped(_)
        | CodingAgentError::BudgetExhausted { .. }
        | CodingAgentError::ModelUnavailable { .. }
        | CodingAgentError::CodebaseMemory(_)
        | CodingAgentError::Parse { .. } => FailureClass::Transient,
        CodingAgentError::NoProduct
        | CodingAgentError::DecisionAnchorRecoveryExhausted
        | CodingAgentError::UndeclaredVerdict { .. }
        | CodingAgentError::InvalidVerdictResult(_) => FailureClass::Permanent,
    }
}

#[cfg(test)]
#[path = "agent_runner/cancellation_tests.rs"]
mod cancellation_tests;

#[cfg(test)]
#[path = "agent_runner/model_failure_tests.rs"]
mod model_failure_tests;

#[cfg(test)]
#[path = "agent_runner/quota_diagnostics_tests.rs"]
mod quota_diagnostics_tests;

#[cfg(test)]
mod tests;
