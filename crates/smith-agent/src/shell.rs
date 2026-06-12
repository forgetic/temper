//! The agent loop's imperative shell.
//!
//! [`AgentShell`] implements [`smith_io_engine::Executor`] for
//! [`AgentMachine`](crate::machine::AgentMachine): it performs the two I/O
//! seams the loop has — streaming a model response and executing a tool — by
//! reusing pi-SDK [`Provider`]s and [`Tool`]s, and feeds every result back into
//! the completion queue. Observability events the machine emits as data are
//! forwarded to a sink; the terminal `Finished` request resolves the run's
//! outcome through a oneshot.
//!
//! The shell never calls into the machine; it only spawns I/O and enqueues
//! completions, keeping the loop's logic single-owner and deterministic.

use std::sync::Arc;

use futures::StreamExt;
use pi::model::{AssistantMessage, Message, StopReason, StreamEvent};
use pi::provider::{Context, Provider, StreamOptions, ToolDef};
use pi::tools::ToolRegistry;
use smith_io_engine::{CqSender, Executor};

use crate::machine::{
    AgentCompletion, AgentEvent, AgentMachine, AgentRequest, AgentStop, StreamDelta,
};

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

/// Performs the agent loop's I/O on the asupersync runtime.
pub struct AgentShell {
    handle: asupersync::runtime::RuntimeHandle,
    cq: CqSender<AgentCompletion>,
    provider: Arc<dyn Provider>,
    tools: Arc<ToolRegistry>,
    system_prompt: Option<String>,
    tool_defs: Arc<Vec<ToolDef>>,
    stream_options: Arc<StreamOptions>,
    events: Arc<dyn EventSink>,
    /// Resolved once, when the machine emits `Finished`.
    outcome: std::sync::Mutex<Option<smith_io_engine::OneshotSender<AgentOutcome>>>,
}

impl AgentShell {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        handle: asupersync::runtime::RuntimeHandle,
        cq: CqSender<AgentCompletion>,
        provider: Arc<dyn Provider>,
        tools: Arc<ToolRegistry>,
        system_prompt: Option<String>,
        tool_defs: Arc<Vec<ToolDef>>,
        stream_options: Arc<StreamOptions>,
        events: Arc<dyn EventSink>,
        outcome: smith_io_engine::OneshotSender<AgentOutcome>,
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
            outcome: std::sync::Mutex::new(Some(outcome)),
        }
    }
}

impl Executor<AgentMachine> for AgentShell {
    fn execute(&self, request: AgentRequest) {
        match request {
            AgentRequest::CallLlm { messages } => {
                let provider = Arc::clone(&self.provider);
                let system_prompt = self.system_prompt.clone();
                let tool_defs = Arc::clone(&self.tool_defs);
                let stream_options = Arc::clone(&self.stream_options);
                let events = Arc::clone(&self.events);
                let cq = self.cq.clone();
                self.handle.spawn(async move {
                    let completion = stream_to_completion(
                        provider.as_ref(),
                        system_prompt.as_deref(),
                        &messages,
                        &tool_defs,
                        &stream_options,
                        events.as_ref(),
                    )
                    .await;
                    let _ = cq.send(completion);
                });
            }
            AgentRequest::RunTool(call) => {
                let tools = Arc::clone(&self.tools);
                let cq = self.cq.clone();
                self.handle.spawn(async move {
                    let output = match tools.get(&call.name) {
                        Some(tool) => match tool.execute(&call.id, call.arguments.clone(), None).await
                        {
                            Ok(output) => output,
                            Err(error) => tool_error_output(&format!(
                                "tool `{}` failed: {error}",
                                call.name
                            )),
                        },
                        None => tool_error_output(&format!("unknown tool `{}`", call.name)),
                    };
                    let _ = cq.send(AgentCompletion::ToolFinished {
                        id: call.id,
                        output,
                    });
                });
            }
            AgentRequest::Emit(event) => {
                self.events.emit(event);
            }
            AgentRequest::Finished {
                stop,
                final_message,
                messages,
            } => {
                if let Some(sender) = self.outcome.lock().expect("outcome lock").take() {
                    sender.send(AgentOutcome {
                        stop,
                        final_message,
                        messages,
                    });
                }
            }
        }
    }
}

/// Streams one model response and collapses it into a completion. The terminal
/// `Done` / `Error` stream event carries the final assistant message; a
/// transport-layer failure (the provider call itself erroring, or the stream
/// ending without a terminal event) becomes [`AgentCompletion::LlmFailed`].
async fn stream_to_completion(
    provider: &dyn Provider,
    system_prompt: Option<&str>,
    messages: &[Message],
    tool_defs: &[ToolDef],
    stream_options: &StreamOptions,
    events: &dyn EventSink,
) -> AgentCompletion {
    let context = Context {
        system_prompt: system_prompt.map(std::borrow::Cow::Borrowed),
        messages: std::borrow::Cow::Borrowed(messages),
        tools: std::borrow::Cow::Borrowed(tool_defs),
    };

    let mut stream = match provider.stream(&context, stream_options).await {
        Ok(stream) => stream,
        Err(error) => return AgentCompletion::LlmFailed(error.to_string()),
    };

    let mut final_message: Option<AssistantMessage> = None;
    while let Some(event) = stream.next().await {
        match event {
            Ok(StreamEvent::Done { message, .. }) => {
                final_message = Some(message);
                break;
            }
            Ok(StreamEvent::Error { error, .. }) => {
                // The provider produced a terminal error message; surface it as
                // an assistant message with an error stop reason so the machine
                // records it and stops cleanly.
                let mut message = error;
                message.stop_reason = StopReason::Error;
                final_message = Some(message);
                break;
            }
            // Live deltas: forwarded to the event sink so observers see tokens
            // and tool calls arrive in real time. They do not affect the loop's
            // decision (the terminal event carries the assembled message), so
            // the machine never sees them — the streaming layer is purely the
            // shell's responsibility.
            Ok(StreamEvent::TextDelta { delta, .. }) => {
                events.emit(AgentEvent::StreamDelta(StreamDelta::Text(delta)));
            }
            Ok(StreamEvent::ThinkingDelta { delta, .. }) => {
                events.emit(AgentEvent::StreamDelta(StreamDelta::Thinking(delta)));
            }
            Ok(StreamEvent::ToolCallEnd { tool_call, .. }) => {
                events.emit(AgentEvent::StreamDelta(StreamDelta::ToolCall {
                    id: tool_call.id,
                    name: tool_call.name,
                }));
            }
            Ok(_) => {}
            Err(error) => return AgentCompletion::LlmFailed(error.to_string()),
        }
    }

    match final_message {
        Some(message) => AgentCompletion::LlmResponded(message),
        None => AgentCompletion::LlmFailed(
            "model stream ended without a terminal Done/Error event".to_string(),
        ),
    }
}

/// Builds an error [`ToolOutput`] carrying `message` as text.
fn tool_error_output(message: &str) -> pi::tools::ToolOutput {
    pi::tools::ToolOutput {
        content: vec![pi::model::ContentBlock::Text(pi::model::TextContent {
            text: message.to_string(),
            text_signature: None,
        })],
        details: None,
        is_error: true,
    }
}
