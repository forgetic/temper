//! Out-of-process agent runner — the production agent boundary.
//!
//! Spawns an agent **program** (the `temper-agent` binary by default, or any
//! operator-provided coder) that speaks the `smith-agent-protocol`:
//!
//! - the worker writes the [`WorkspaceContext`] to a temp file and passes its
//!   path as the `--context` flag, the result path as `--result`, and the
//!   prepared coordination-scoped workspace root as `--workspace` (also the
//!   child's cwd);
//! - the program writes a [`WorkspaceResult`] to the file named by `--result`,
//!   which the worker reads back.
//!
//! This replaces the former in-process pi-SDK runner: the worker links no
//! agent/LLM code, only this protocol. It also subsumes the old
//! `ExternalCommandRunner` (same file protocol).
//!
//! The child is owned by a dedicated joined supervisor thread. The async run
//! polls process completion, side channels, and the worker cancellation
//! handshake together; graceful, forced, and hard-kill requests are forwarded
//! distinctly and the joined supervisor outcome is returned to WorkerMachine.
//! Dropping remains only an abrupt component-loss hard-kill safety net.

use std::future::Future;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;
use std::task::Poll;

use temper_process_containment::{ContainmentFactory, ContainmentScope};
use temper_protocol_activity::{
    ACTIVITY_ADDRESS_FLAG, AgentActivityCapturePolicyV1, TRACE_POLICY_FLAG,
};
use temper_protocol_agent::{
    AGENT_LIFECYCLE_ADDRESS_FLAG, AgentRuntimeLimitsV1, AgentToolConfig,
    FORGE_CONTEXT_ADDRESS_FLAG, ForgeContextResponse, RUNTIME_LIMITS_FLAG,
    SUBMIT_FOR_PR_ADDRESS_FLAG, SubmitForPrRequest, SubmitForPrResponse, TOOL_CONFIG_FLAG,
    WorkspaceContext,
};

use crate::agent_runner::{
    AcceptedSubmitProofStore, AgentForgeContextFuture, AgentForgeContextHost, AgentRunError,
    AgentRunOutput, WorkspaceResult,
};
use crate::executor::{AttemptFence, JobCancellation};
use crate::trace::{TraceCollector, TraceRun};
use crate::{WorkerAgentTraceConfig, WorkerLivenessLimits};

mod command;
mod lifecycle;
mod runner;
mod runtime_limits;
mod side_channel;
mod stderr;
mod supervisor;
mod terminal;
pub use crate::executor::CancellationOutcome;
use side_channel::{
    ForgeSideChannelRequest, LocalServer, SubmitSideChannelRequest, start_forge_server,
    start_submit_server, submit_for_pr_available,
};
use stderr::DiagnosticIdentity;
#[cfg(test)]
use stderr::stderr_tail;
pub use supervisor::JobQuiesced;
use supervisor::{ManagedAgentProcess, SupervisorResult};

/// Host-side submit gate used by the out-of-process carrier.
type SubmitForPrFuture = Pin<Box<dyn Future<Output = SubmitForPrResponse> + Send + 'static>>;
type SubmitForPrHandler = Arc<
    dyn Fn(SubmitForPrRequest, WorkspaceContext, PathBuf, JobCancellation) -> SubmitForPrFuture
        + Send
        + Sync,
>;

fn default_submit_for_pr_handler() -> SubmitForPrHandler {
    Arc::new(|request, context, cwd, cancellation| {
        Box::pin(async move {
            crate::pre_push::submit_for_pr_pre_push_response_controlled(
                &request,
                &context,
                cwd,
                cancellation,
            )
            .await
        })
    })
}

type ContainmentFactoryProvider =
    Arc<dyn Fn(&str, &str) -> std::io::Result<ContainmentFactory> + Send + Sync>;

fn default_containment_factory_provider() -> ContainmentFactoryProvider {
    Arc::new(crate::process_containment::production_factory)
}

/// Spawns an agent program speaking the `smith-agent-protocol`.
#[derive(Clone)]
pub struct OutOfProcessRunner {
    /// Program followed by fixed arguments, e.g.
    /// `["temper", "agent", "--provider", "anthropic", "--model", "…"]`. The
    /// per-job `--context`/`--result`/`--workspace` flags are appended at spawn.
    command: Vec<String>,
    /// Environment injected into every spawned agent (on top of the inherited
    /// environment): just the one secret provider-credential var, which a
    /// config-driven worker passes explicitly rather than relying on its own
    /// inherited environment.
    env: Vec<(String, String)>,
    /// Non-secret agent-local tool settings. When present and enabled for the
    /// current workflow role, these are written to a per-run JSON file and
    /// passed as `--tool-config <file>`.
    tool_config: Option<AgentToolConfig>,
    /// Complete operation limits supplied only to known first-party agents.
    runtime_limits: Option<AgentRuntimeLimitsV1>,
    /// Worker-owned cancellation limits. WorkerMachine uses the same resolved
    /// values to schedule the explicit soft- and hard-escalation requests.
    liveness_limits: WorkerLivenessLimits,
    /// Shared, non-secret capture policy written to a per-run JSON file for the
    /// first-party agent process. `None` preserves third-party agent compatibility.
    trace_policy: Option<AgentActivityCapturePolicyV1>,
    /// Worker-owned durable collector. It runs for every invocation, including
    /// third-party children that never connect to an activity endpoint.
    trace_collector: TraceCollector,
    /// Host-controlled submit gate serviced over a worker-owned local channel
    /// while the child process remains alive.
    submit_for_pr: SubmitForPrHandler,
    /// Optional authenticated, assignment-bound read-only Forge host.
    forge_context: Option<AgentForgeContextHost>,
    /// Attempt-scoped descendant-complete containment composition.
    containment_factory: ContainmentFactoryProvider,
    /// Unit-test override used to verify diagnostics emitted from the blocking
    /// pool without installing a process-global subscriber.
    #[cfg(test)]
    diagnostic_dispatch: Option<tracing::Dispatch>,
}

impl std::fmt::Debug for OutOfProcessRunner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OutOfProcessRunner")
            .field("command", &self.command)
            .field(
                "env",
                &self.env.iter().map(|(key, _)| key).collect::<Vec<_>>(),
            )
            .field("tool_config", &self.tool_config)
            .field("runtime_limits", &self.runtime_limits)
            .field("liveness_limits", &self.liveness_limits)
            .field("trace_policy", &self.trace_policy)
            .field("trace_collector", &self.trace_collector)
            .field("submit_for_pr", &"<handler>")
            .field(
                "forge_context",
                &self.forge_context.as_ref().map(|_| "<host>"),
            )
            .field("containment_factory", &"<factory>")
            .finish()
    }
}

impl OutOfProcessRunner {
    /// Builds a runner for the given command (program first, then args).
    pub fn new(command: Vec<String>) -> Self {
        Self {
            command,
            env: Vec::new(),
            tool_config: None,
            runtime_limits: None,
            liveness_limits: WorkerLivenessLimits::default(),
            trace_policy: None,
            trace_collector: TraceCollector::default(),
            submit_for_pr: default_submit_for_pr_handler(),
            forge_context: None,
            containment_factory: default_containment_factory_provider(),
            #[cfg(test)]
            diagnostic_dispatch: None,
        }
    }

    /// Sets the environment injected into every spawned agent.
    #[must_use]
    pub fn with_env(mut self, env: Vec<(String, String)>) -> Self {
        self.env = env;
        self
    }

    /// Sets the non-secret agent tool config written per run when enabled for
    /// the assigned workflow role.
    #[must_use]
    pub fn with_tool_config(mut self, tool_config: Option<AgentToolConfig>) -> Self {
        self.tool_config = tool_config;
        self
    }

    /// Sets complete operation limits for a known first-party agent. Leaving
    /// this unset preserves third-party command compatibility.
    #[must_use]
    pub fn with_runtime_limits(mut self, runtime_limits: Option<AgentRuntimeLimitsV1>) -> Self {
        self.runtime_limits = runtime_limits;
        self
    }

    /// Sets worker-owned process cancellation and escalation bounds.
    #[must_use]
    pub fn with_liveness_limits(mut self, liveness_limits: WorkerLivenessLimits) -> Self {
        self.liveness_limits = liveness_limits;
        self
    }

    /// Sets the non-secret trace capture policy written for first-party agents.
    #[must_use]
    pub fn with_trace_policy(mut self, trace_policy: Option<AgentActivityCapturePolicyV1>) -> Self {
        self.trace_policy = trace_policy;
        self
    }

    /// Configures worker-owned run collection from configuration.
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

    /// Overrides the host-controlled `submit_for_pr` gate serviced for writable
    /// engineer sessions.
    #[must_use]
    pub fn with_submit_for_pr_handler<F>(mut self, handler: F) -> Self
    where
        F: Fn(SubmitForPrRequest, &WorkspaceContext, &Path) -> SubmitForPrResponse
            + Send
            + Sync
            + 'static,
    {
        self.submit_for_pr = Arc::new(move |request, context, cwd, _cancellation| {
            let response = handler(request, &context, &cwd);
            Box::pin(std::future::ready(response))
        });
        self
    }

    /// Overrides the submit gate with an asynchronous, cancellation-aware host.
    #[must_use]
    pub fn with_async_submit_for_pr_handler<F, Fut>(mut self, handler: F) -> Self
    where
        F: Fn(SubmitForPrRequest, WorkspaceContext, PathBuf) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = SubmitForPrResponse> + Send + 'static,
    {
        self.submit_for_pr = Arc::new(move |request, context, cwd, _cancellation| {
            Box::pin(handler(request, context, cwd))
        });
        self
    }

    /// Installs the worker-owned assignment-bound Forge read host.
    #[must_use]
    pub fn with_forge_context_host(mut self, host: AgentForgeContextHost) -> Self {
        self.forge_context = Some(host);
        self
    }

    /// Overrides attempt containment selection without process-global state.
    #[must_use]
    pub fn with_containment_factory<F>(mut self, factory: F) -> Self
    where
        F: Fn(&str, &str) -> std::io::Result<ContainmentFactory> + Send + Sync + 'static,
    {
        self.containment_factory = Arc::new(factory);
        self
    }

    /// Scope production cgroup ownership to this logical worker. The factory
    /// also records the current process boot, so concurrent incarnations of the
    /// same worker id remain independently liveness-fenced.
    #[must_use]
    pub fn with_containment_owner(mut self, owner: impl Into<String>) -> Self {
        let owner = owner.into();
        self.containment_factory = Arc::new(move |job, attempt| {
            crate::process_containment::production_factory_for_owner(&owner, job, attempt)
        });
        self
    }

    #[cfg(test)]
    fn with_diagnostic_dispatch(mut self, dispatch: tracing::Dispatch) -> Self {
        self.diagnostic_dispatch = Some(dispatch);
        self
    }

    fn diagnostic_dispatch(&self) -> tracing::Dispatch {
        #[cfg(test)]
        if let Some(dispatch) = &self.diagnostic_dispatch {
            return dispatch.clone();
        }
        tracing::dispatcher::get_default(|dispatch| dispatch.clone())
    }
}

/// What the joined child supervisor produced.
struct ChildOutcome {
    /// Process exit code (`None` if terminated by signal without a code).
    status_code: Option<i32>,
    /// Last bytes of captured stderr, for error messages.
    stderr_tail: String,
}

mod managed_run;

#[cfg(test)]
#[path = "out_of_process_runner_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "out_of_process_runner_lifecycle_tests.rs"]
mod lifecycle_tests;

#[cfg(test)]
#[path = "out_of_process_runner_supervisor_tests.rs"]
mod supervisor_tests;

#[cfg(test)]
#[path = "out_of_process_runner_trace_tests.rs"]
mod trace_tests;

#[cfg(test)]
#[path = "out_of_process_runner_stderr_tests.rs"]
mod stderr_tests;
