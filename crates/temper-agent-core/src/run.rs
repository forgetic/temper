//! Running a sub-agent: wire the machine + shell and drive them.
//!
//! A **sub-agent** is one bounded LLM run defined by a [`SubAgent`]: a system
//! prompt, an initial user message, a tool set (optionally workspace-scoped),
//! an iteration budget, and the provider/stream options. [`run_sub_agent`]
//! builds the pure [`AgentMachine`], the imperative [`AgentShell`], and a
//! completion queue, drives them with [`temper_agent_io::drive`], and returns
//! the settled [`AgentOutcome`].
//!
//! Must run inside an engine task (the drive loop reads the runtime clock and
//! the shell spawns I/O), so callers wrap it in [`temper_agent_io::block_on`]
//! or call it from another engine task.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::Duration;

use skein::runtime::RuntimeHandle;
use temper_agent_io::{CqSender, channel, drive, oneshot};
use tongs::model::{Message, UserContent, UserMessage};
use tongs::provider::{Provider, StreamOptions, ToolDef};
use tongs::tools::ToolRegistry;
use tongs::tools::tool_to_definition;

use crate::machine::{AgentCompletion, AgentEvent, AgentMachine, ArgPreviewFn};
use crate::shell::{
    AgentOutcome, AgentShell, EventSink, ModelIdentity, NullEventSink, RunObservability, TurnHook,
};

/// Contains observer panics at the core boundary. Observability is best effort:
/// neither prompt capture nor a later lifecycle callback may change a run.
struct PanicSafeEventSink {
    inner: Arc<dyn EventSink>,
}

impl EventSink for PanicSafeEventSink {
    fn emit(&self, event: AgentEvent) {
        let _ = catch_unwind(AssertUnwindSafe(|| self.inner.emit(event)));
    }
}

/// A live control handle for a running sub-agent: inject steering messages or
/// abort it from outside the run.
///
/// The handle wraps a clone of the run's completion-queue sender, so it can be
/// moved to another task/thread (it is `Send + Clone`) and used while the run is
/// in flight. Steering is applied at the next turn boundary; abort drains any
/// in-flight tool batch and then stops the run with [`AgentStop::Aborted`].
/// Calls after the run has finished are harmless no-ops (the queue is closed).
#[derive(Clone)]
pub struct SubAgentControl {
    cq: CqSender<AgentCompletion>,
}

impl SubAgentControl {
    /// Inject steering messages, applied at the next turn boundary.
    pub fn steer(&self, messages: Vec<Message>) {
        let _ = self.cq.send(AgentCompletion::Steer(messages));
    }

    /// Inject a plain-text steering message.
    pub fn steer_text(&self, text: impl Into<String>) {
        self.steer(vec![Message::User(UserMessage {
            content: UserContent::Text(text.into()),
            timestamp: 0,
        })]);
    }

    /// Abort the run. The current tool batch (if any) drains first, then the run
    /// stops with [`crate::AgentStop::Aborted`].
    pub fn abort(&self) {
        let _ = self.cq.send(AgentCompletion::Abort);
    }
}

/// Complete operation deadlines carried by every main or nested agent run.
/// Deadline enforcement is owned by the shell; this type keeps the resolved
/// contract beside the run definition without coupling the core to config or
/// process-protocol crates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentOperationLimits {
    pub tool_timeout: Duration,
    pub model_connect_timeout: Duration,
    pub model_idle_timeout: Duration,
}

impl Default for AgentOperationLimits {
    fn default() -> Self {
        Self {
            tool_timeout: Duration::from_secs(600),
            model_connect_timeout: Duration::from_secs(120),
            model_idle_timeout: Duration::from_secs(120),
        }
    }
}

/// The definition of one sub-agent run.
pub struct SubAgent {
    /// The role/system prompt that frames the run.
    pub system_prompt: Option<String>,
    /// The initial user message (the task).
    pub user_message: String,
    /// The tools the agent may call, optionally already scoped to a workspace
    /// `cwd` by the caller (e.g. pi's `create_read_tool(cwd)` et al.).
    pub tools: ToolRegistry,
    /// Ceiling on tool-using iterations.
    pub max_iterations: usize,
    /// Complete model/tool operation deadlines inherited by nested runs.
    pub operation_limits: AgentOperationLimits,
    /// The model provider.
    pub provider: Arc<dyn Provider>,
    /// Per-request stream options (api key/bearer, headers, temperature,
    /// thinking level). The caller resolves the bearer before the run.
    pub stream_options: StreamOptions,
}

/// Why a sub-agent could not be started or driven.
#[derive(Debug)]
pub enum SubAgentError {
    /// `run_sub_agent` was not called on an engine runtime.
    RuntimeUnavailable,
    /// The drive loop ended without the machine producing an outcome (should not
    /// happen — the machine always finishes).
    NoOutcome,
}

impl std::fmt::Display for SubAgentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SubAgentError::RuntimeUnavailable => {
                formatter.write_str("run_sub_agent must be driven on an skein engine runtime")
            }
            SubAgentError::NoOutcome => {
                formatter.write_str("sub-agent drive loop ended without an outcome")
            }
        }
    }
}

impl std::error::Error for SubAgentError {}

/// Runs a sub-agent to completion with no event sink.
///
/// `handle` is the runtime's spawn capability, passed explicitly from the
/// caller's engine context (no ambient handle lookup).
pub async fn run_sub_agent(
    handle: RuntimeHandle,
    sub_agent: SubAgent,
) -> Result<AgentOutcome, SubAgentError> {
    run_sub_agent_with_events(handle, sub_agent, Arc::new(NullEventSink)).await
}

/// Runs a sub-agent to completion with a [`TurnHook`] awaited before each
/// model call and no event sink.
pub async fn run_sub_agent_with_hook(
    handle: RuntimeHandle,
    sub_agent: SubAgent,
    turn_hook: Arc<dyn TurnHook>,
) -> Result<AgentOutcome, SubAgentError> {
    let (_control, run) = run_sub_agent_controllable_with_hook(
        handle,
        sub_agent,
        Arc::new(NullEventSink),
        Some(turn_hook),
    )?;
    run.await
}

/// Runs a sub-agent to completion, forwarding observability events to `events`.
pub async fn run_sub_agent_with_events(
    handle: RuntimeHandle,
    sub_agent: SubAgent,
    events: Arc<dyn EventSink>,
) -> Result<AgentOutcome, SubAgentError> {
    let (_control, run) = run_sub_agent_controllable(handle, sub_agent, events)?;
    run.await
}

/// Builds a controllable sub-agent run: returns a [`SubAgentControl`] handle for
/// live steering/abort plus a future that drives the run to its
/// [`AgentOutcome`]. The control is available *before* the run is awaited, so a
/// caller can hand it to another task/thread and steer or abort while the run is
/// in flight, e.g.:
///
/// ```ignore
/// let (control, run) = run_sub_agent_controllable(handle, sub_agent, events)?;
/// handle.spawn(async move { /* … */ control.abort(); });
/// let outcome = run.await?;
/// ```
///
/// `handle` is the runtime's spawn capability, passed explicitly.
pub fn run_sub_agent_controllable(
    handle: RuntimeHandle,
    sub_agent: SubAgent,
    events: Arc<dyn EventSink>,
) -> Result<
    (
        SubAgentControl,
        impl std::future::Future<Output = Result<AgentOutcome, SubAgentError>>,
    ),
    SubAgentError,
> {
    run_sub_agent_controllable_with_hook(handle, sub_agent, events, None)
}

/// [`run_sub_agent_controllable`] with an optional [`TurnHook`] awaited
/// before each model call.
pub fn run_sub_agent_controllable_with_hook(
    handle: RuntimeHandle,
    sub_agent: SubAgent,
    events: Arc<dyn EventSink>,
    turn_hook: Option<Arc<dyn TurnHook>>,
) -> Result<
    (
        SubAgentControl,
        impl std::future::Future<Output = Result<AgentOutcome, SubAgentError>>,
    ),
    SubAgentError,
> {
    run_sub_agent_controllable_with_hooks(handle, sub_agent, events, turn_hook, None)
}

/// [`run_sub_agent_controllable_with_hook`] with an additional optional
/// [`ArgPreviewFn`] used to fill `ToolStart.arg_preview` from each call's name +
/// arguments. The preview function lives above the core tier (it knows the
/// workspace `cwd` and per-tool rendering rules); the pure machine just calls
/// it. See the agent-log-cleanup plan (pieces B/D).
pub fn run_sub_agent_controllable_with_hooks(
    handle: RuntimeHandle,
    sub_agent: SubAgent,
    events: Arc<dyn EventSink>,
    turn_hook: Option<Arc<dyn TurnHook>>,
    arg_preview: Option<ArgPreviewFn>,
) -> Result<
    (
        SubAgentControl,
        impl std::future::Future<Output = Result<AgentOutcome, SubAgentError>>,
    ),
    SubAgentError,
> {
    let model = ModelIdentity::new(sub_agent.provider.api(), "unknown");
    run_sub_agent_controllable_with_observability(
        handle,
        sub_agent,
        RunObservability::new(events, model),
        turn_hook,
        arg_preview,
    )
}

/// Full run builder with explicit model identity, event sink, and monotonic
/// timing clock. Agent-tier callers use this seam to produce canonical activity;
/// legacy callers retain the default observer through
/// [`run_sub_agent_controllable_with_hooks`].
pub fn run_sub_agent_controllable_with_observability(
    handle: RuntimeHandle,
    sub_agent: SubAgent,
    observability: RunObservability,
    turn_hook: Option<Arc<dyn TurnHook>>,
    arg_preview: Option<ArgPreviewFn>,
) -> Result<
    (
        SubAgentControl,
        impl std::future::Future<Output = Result<AgentOutcome, SubAgentError>>,
    ),
    SubAgentError,
> {
    let tool_defs: Vec<ToolDef> = sub_agent
        .tools
        .tools()
        .iter()
        .map(|tool| tool_to_definition(tool.as_ref()))
        .collect();

    // Effect map for parallel batching: each tool declares its effects, which
    // the machine uses to plan which adjacent tool calls may run concurrently.
    let effects: std::collections::BTreeMap<String, tongs::tools::ToolEffects> = sub_agent
        .tools
        .tools()
        .iter()
        .map(|tool| (tool.name().to_string(), tool.effects()))
        .collect();

    let initial = vec![Message::User(UserMessage {
        content: UserContent::Text(sub_agent.user_message.clone()),
        timestamp: 0,
    })];

    let RunObservability {
        events,
        model,
        clock,
    } = observability;
    let events: Arc<dyn EventSink> = Arc::new(PanicSafeEventSink { inner: events });
    let observability = RunObservability {
        events: Arc::clone(&events),
        model,
        clock,
    };
    events.emit(AgentEvent::PromptPrepared {
        system_prompt: sub_agent.system_prompt.clone(),
        initial_user_message: sub_agent.user_message.clone(),
        tools: tool_defs.clone(),
    });

    let (cq_tx, cq_rx) = channel();
    let (outcome_tx, outcome_rx) = oneshot();

    // The control handle is a clone of the completion sender — steering/abort
    // are just completions the machine already knows how to handle.
    let control = SubAgentControl { cq: cq_tx.clone() };

    let mut shell = AgentShell::new(
        handle,
        cq_tx,
        sub_agent.provider,
        Arc::new(sub_agent.tools),
        sub_agent.system_prompt,
        Arc::new(tool_defs),
        Arc::new(sub_agent.stream_options),
        observability,
        outcome_tx,
    );
    if let Some(turn_hook) = turn_hook {
        shell = shell.with_turn_hook(turn_hook);
    }
    let mut machine = AgentMachine::with_effects(initial, sub_agent.max_iterations, effects);
    if let Some(arg_preview) = arg_preview {
        machine = machine.with_arg_preview(arg_preview);
    }

    let run = async move {
        // Drive to completion. The machine stops itself on `Finished`, which
        // also resolves the outcome oneshot.
        let _ = drive(machine, &shell, cq_rx).await;
        outcome_rx.recv().await.ok_or(SubAgentError::NoOutcome)
    };
    Ok((control, run))
}
