//! The agent shell's I/O orchestration: the public shell types and the
//! [`Executor`] that performs the loop's two I/O seams.
//!
//! [`AgentShell`] spawns model streaming and tool execution on the skein
//! runtime and feeds every result back into the completion queue, never calling
//! into the machine. Observability events are forwarded to an [`EventSink`]; the
//! terminal `Finished` request resolves the run's outcome through a oneshot.

use std::sync::Arc;

use temper_agent_io::{CqSender, Executor};
use tongs::model::{AssistantMessage, Message};
use tongs::provider::{Provider, StreamOptions, ToolDef};
use tongs::tools::ToolRegistry;

use crate::machine::{AgentCompletion, AgentEvent, AgentMachine, AgentRequest, AgentStop};
use crate::shell::streaming::stream_to_completion;

/// The settled result of a sub-agent run.
#[derive(Clone, Debug)]
pub struct AgentOutcome {
    pub stop: AgentStop,
    pub final_message: AssistantMessage,
    pub messages: Vec<Message>,
}

/// A sink for observability events. The default just drops them; callers that
/// want a live view (a TUI, a log, a transcript recorder) supply their own.
pub trait EventSink: Send + Sync {
    fn emit(&self, event: AgentEvent);
}

/// An [`EventSink`] that discards events.
pub struct NullEventSink;

impl EventSink for NullEventSink {
    fn emit(&self, _event: AgentEvent) {}
}

/// An async hook the shell awaits immediately before each model call.
///
/// A model call only starts once the previous turn's tool batch has fully
/// drained (the machine sequences `CallLlm` strictly after the batch), so the
/// hook runs with **no tool in flight** — the natural coherent step boundary
/// for committing and pushing a workspace checkpoint (phase 6b). `turn` is
/// zero-based; the first model call of a run is turn 0 (nothing has happened
/// yet, so checkpoint hooks typically skip it).
///
/// The hook must not fail the run: it returns nothing and implementations
/// swallow their own errors.
#[async_trait::async_trait]
pub trait TurnHook: Send + Sync {
    async fn before_model_call(&self, turn: usize);
}

/// Performs the agent loop's I/O on the skein runtime.
pub struct AgentShell {
    handle: skein::runtime::RuntimeHandle,
    cq: CqSender<AgentCompletion>,
    provider: Arc<dyn Provider>,
    tools: Arc<ToolRegistry>,
    system_prompt: Option<String>,
    tool_defs: Arc<Vec<ToolDef>>,
    stream_options: Arc<StreamOptions>,
    events: Arc<dyn EventSink>,
    /// Awaited before each model call (turn boundary); see [`TurnHook`].
    turn_hook: Option<Arc<dyn TurnHook>>,
    /// Zero-based count of model calls dispatched, for the hook's `turn`.
    turns_started: std::sync::atomic::AtomicUsize,
    /// Resolved once, when the machine emits `Finished`.
    outcome: std::sync::Mutex<Option<temper_agent_io::OneshotSender<AgentOutcome>>>,
}

impl AgentShell {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        handle: skein::runtime::RuntimeHandle,
        cq: CqSender<AgentCompletion>,
        provider: Arc<dyn Provider>,
        tools: Arc<ToolRegistry>,
        system_prompt: Option<String>,
        tool_defs: Arc<Vec<ToolDef>>,
        stream_options: Arc<StreamOptions>,
        events: Arc<dyn EventSink>,
        outcome: temper_agent_io::OneshotSender<AgentOutcome>,
    ) -> Self {
        Self {
            handle,
            cq,
            provider,
            tools,
            system_prompt,
            tool_defs,
            stream_options,
            events,
            turn_hook: None,
            turns_started: std::sync::atomic::AtomicUsize::new(0),
            outcome: std::sync::Mutex::new(Some(outcome)),
        }
    }

    /// Installs a [`TurnHook`] awaited before each model call.
    pub fn with_turn_hook(mut self, turn_hook: Arc<dyn TurnHook>) -> Self {
        self.turn_hook = Some(turn_hook);
        self
    }

    /// Spawn the model call for one turn: await the turn hook, stream the
    /// response, emit per-turn usage, and enqueue the completion.
    fn execute_call_llm(&self, messages: Vec<Message>) {
        let provider = Arc::clone(&self.provider);
        let system_prompt = self.system_prompt.clone();
        let tool_defs = Arc::clone(&self.tool_defs);
        let stream_options = Arc::clone(&self.stream_options);
        let events = Arc::clone(&self.events);
        let cq = self.cq.clone();
        let turn_hook = self.turn_hook.clone();
        let turn = self
            .turns_started
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.handle.spawn(async move {
            if let Some(hook) = turn_hook {
                hook.before_model_call(turn).await;
            }
            let completion = stream_to_completion(
                provider.as_ref(),
                system_prompt.as_deref(),
                &messages,
                &tool_defs,
                &stream_options,
                events.as_ref(),
            )
            .await;
            // Per-turn token accounting, emitted as soon as the turn's terminal
            // message lands (both normal and error stops).
            if let AgentCompletion::LlmResponded(message) = &completion {
                events.emit(AgentEvent::TurnUsage {
                    turn,
                    usage: message.usage,
                });
            }
            let _ = cq.send(completion);
        });
    }

    /// Spawn one tool execution, mapping a missing/erroring tool to an error
    /// [`tongs::tools::ToolOutput`], and enqueue the completion.
    fn execute_run_tool(&self, call: tongs::model::ToolCall) {
        let tools = Arc::clone(&self.tools);
        let cq = self.cq.clone();
        self.handle.spawn(async move {
            let output = match tools.get(&call.name) {
                Some(tool) => match tool.execute(&call.id, call.arguments.clone(), None).await {
                    Ok(output) => output,
                    Err(error) => {
                        tool_error_output(&format!("tool `{}` failed: {error}", call.name))
                    }
                },
                None => tool_error_output(&format!("unknown tool `{}`", call.name)),
            };
            let _ = cq.send(AgentCompletion::ToolFinished {
                id: call.id,
                output,
            });
        });
    }

    /// Resolve the run's outcome oneshot when the machine finishes.
    fn execute_finished(
        &self,
        stop: AgentStop,
        final_message: AssistantMessage,
        messages: Vec<Message>,
    ) {
        if let Some(sender) = self.outcome.lock().expect("outcome lock").take() {
            sender.send(AgentOutcome {
                stop,
                final_message,
                messages,
            });
        }
    }
}

impl Executor<AgentMachine> for AgentShell {
    fn execute(&self, request: AgentRequest) {
        match request {
            AgentRequest::CallLlm { messages } => self.execute_call_llm(messages),
            AgentRequest::RunTool(call) => self.execute_run_tool(call),
            AgentRequest::Emit(event) => self.events.emit(event),
            AgentRequest::Finished {
                stop,
                final_message,
                messages,
            } => self.execute_finished(stop, final_message, messages),
        }
    }
}

/// Builds an error [`tongs::tools::ToolOutput`] carrying `message` as text.
fn tool_error_output(message: &str) -> tongs::tools::ToolOutput {
    tongs::tools::ToolOutput {
        content: vec![tongs::model::ContentBlock::Text(
            tongs::model::TextContent {
                text: message.to_string(),
                text_signature: None,
            },
        )],
        details: None,
        is_error: true,
    }
}
