//! Running a sub-agent: wire the machine + shell and drive them.
//!
//! A **sub-agent** is one bounded LLM run defined by a [`SubAgent`]: a system
//! prompt, an initial user message, a tool set (optionally workspace-scoped),
//! an iteration budget, and the provider/stream options. [`run_sub_agent`]
//! builds the pure [`AgentMachine`], the imperative [`AgentShell`], and a
//! completion queue, drives them with [`smith_io_engine::drive`], and returns
//! the settled [`AgentOutcome`].
//!
//! Must run inside an engine task (the drive loop reads the runtime clock and
//! the shell spawns I/O), so callers wrap it in [`smith_io_engine::block_on`]
//! or call it from another engine task.

use std::sync::Arc;

use pi::model::{Message, UserContent, UserMessage};
use pi::provider::{Provider, StreamOptions, ToolDef};
use pi::sdk::tool_to_definition;
use pi::tools::ToolRegistry;
use smith_io_engine::{CqSender, channel, drive, oneshot};

use crate::machine::{AgentCompletion, AgentMachine};
use crate::shell::{AgentOutcome, AgentShell, EventSink, NullEventSink};

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
            SubAgentError::RuntimeUnavailable => formatter.write_str(
                "run_sub_agent must be driven on an asupersync engine runtime",
            ),
            SubAgentError::NoOutcome => {
                formatter.write_str("sub-agent drive loop ended without an outcome")
            }
        }
    }
}

impl std::error::Error for SubAgentError {}

/// Runs a sub-agent to completion with no event sink.
pub async fn run_sub_agent(sub_agent: SubAgent) -> Result<AgentOutcome, SubAgentError> {
    run_sub_agent_with_events(sub_agent, Arc::new(NullEventSink)).await
}

/// Runs a sub-agent to completion, forwarding observability events to `events`.
pub async fn run_sub_agent_with_events(
    sub_agent: SubAgent,
    events: Arc<dyn EventSink>,
) -> Result<AgentOutcome, SubAgentError> {
    let (_control, run) = run_sub_agent_controllable(sub_agent, events)?;
    run.await
}

/// Builds a controllable sub-agent run: returns a [`SubAgentControl`] handle for
/// live steering/abort plus a future that drives the run to its
/// [`AgentOutcome`]. The control is available *before* the run is awaited, so a
/// caller can hand it to another task/thread and steer or abort while the run is
/// in flight, e.g.:
///
/// ```ignore
/// let (control, run) = run_sub_agent_controllable(sub_agent, events)?;
/// handle.spawn(async move { /* … */ control.abort(); });
/// let outcome = run.await?;
/// ```
///
/// Must be called inside an engine task (it needs the runtime handle).
pub fn run_sub_agent_controllable(
    sub_agent: SubAgent,
    events: Arc<dyn EventSink>,
) -> Result<
    (
        SubAgentControl,
        impl std::future::Future<Output = Result<AgentOutcome, SubAgentError>>,
    ),
    SubAgentError,
> {
    let handle = asupersync::runtime::Runtime::current_handle()
        .ok_or(SubAgentError::RuntimeUnavailable)?;

    let tool_defs: Vec<ToolDef> = sub_agent
        .tools
        .tools()
        .iter()
        .map(|tool| tool_to_definition(tool.as_ref()))
        .collect();

    // Effect map for parallel batching: each tool declares its effects, which
    // the machine uses to plan which adjacent tool calls may run concurrently.
    let effects: std::collections::BTreeMap<String, pi::tools::ToolEffects> = sub_agent
        .tools
        .tools()
        .iter()
        .map(|tool| (tool.name().to_string(), tool.effects()))
        .collect();

    let initial = vec![Message::User(UserMessage {
        content: UserContent::Text(sub_agent.user_message),
        timestamp: 0,
    })];

    let (cq_tx, cq_rx) = channel();
    let (outcome_tx, outcome_rx) = oneshot();

    // The control handle is a clone of the completion sender — steering/abort
    // are just completions the machine already knows how to handle.
    let control = SubAgentControl { cq: cq_tx.clone() };

    let shell = AgentShell::new(
        handle,
        cq_tx,
        sub_agent.provider,
        Arc::new(sub_agent.tools),
        sub_agent.system_prompt,
        Arc::new(tool_defs),
        Arc::new(sub_agent.stream_options),
        events,
        outcome_tx,
    );
    let machine = AgentMachine::with_effects(initial, sub_agent.max_iterations, effects);

    let run = async move {
        // Drive to completion. The machine stops itself on `Finished`, which
        // also resolves the outcome oneshot.
        let _ = drive(machine, &shell, cq_rx).await;
        outcome_rx.recv().await.ok_or(SubAgentError::NoOutcome)
    };
    Ok((control, run))
}
