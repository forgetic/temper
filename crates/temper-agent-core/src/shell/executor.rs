//! The agent shell's I/O orchestration: the public shell types and the
//! [`Executor`] that performs the loop's two I/O seams.
//!
//! [`AgentShell`] spawns model streaming and tool execution on the skein
//! runtime and feeds every result back into the completion queue, never calling
//! into the machine. Observability events are forwarded to an [`EventSink`]; the
//! terminal `Finished` request resolves the run's outcome through a oneshot.

use std::sync::Arc;

use temper_agent_io::{CqSender, Executor};
use tongs::model::{AssistantMessage, Message, ToolCall};
use tongs::provider::{Provider, StreamOptions, ToolDef};
use tongs::tools::ToolRegistry;

use crate::machine::{
    AgentCompletion, AgentEvent, AgentMachine, AgentRequest, AgentStop, ToolCallStatus,
    ToolResultMetadata,
};
use crate::shell::streaming::{ModelCallObservability, stream_to_completion};

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

/// Monotonic clock used for model/tool activity timing. Supplying it through
/// the run observer keeps timing deterministic under virtual/fake clocks.
pub trait EventClock: Send + Sync {
    fn now_millis(&self) -> u64;
}

/// Production clock backed by the runtime timer driver.
pub struct SystemEventClock;

impl EventClock for SystemEventClock {
    fn now_millis(&self) -> u64 {
        temper_agent_io::engine_now().as_nanos() / 1_000_000
    }
}

/// Non-secret model identity attached to every model-attempt event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelIdentity {
    pub provider: String,
    pub model: String,
}

impl ModelIdentity {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
        }
    }
}

/// Observability dependencies for one agent invocation.
#[derive(Clone)]
pub struct RunObservability {
    pub events: Arc<dyn EventSink>,
    pub model: ModelIdentity,
    pub clock: Arc<dyn EventClock>,
}

impl RunObservability {
    pub fn new(events: Arc<dyn EventSink>, model: ModelIdentity) -> Self {
        Self {
            events,
            model,
            clock: Arc::new(SystemEventClock),
        }
    }

    #[must_use]
    pub fn with_clock(mut self, clock: Arc<dyn EventClock>) -> Self {
        self.clock = clock;
        self
    }
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
/// hook runs with **no tool in flight** — a natural coherent turn boundary.
/// `turn` is zero-based; the first model call of a run is turn 0.
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
    model: ModelIdentity,
    clock: Arc<dyn EventClock>,
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
        observability: RunObservability,
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
            events: observability.events,
            model: observability.model,
            clock: observability.clock,
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
        let model = self.model.clone();
        let clock = Arc::clone(&self.clock);
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
                ModelCallObservability {
                    turn,
                    model: &model,
                    clock: clock.as_ref(),
                    events: events.as_ref(),
                },
            )
            .await;
            // Preserve the long-standing per-turn usage event. The canonical
            // normalizer turns this into the single usage projection source.
            if let AgentCompletion::LlmResponded(message) = &completion {
                events.emit(AgentEvent::TurnUsage {
                    turn,
                    usage: message.usage,
                });
            }
            let _ = cq.send(completion);
        });
    }

    /// Spawn one tool execution, measuring it on the same monotonic clock used
    /// for model attempts. Event failures cannot affect the completion result.
    fn execute_run_tool(&self, call: tongs::model::ToolCall) {
        let tools = Arc::clone(&self.tools);
        let events = Arc::clone(&self.events);
        let clock = Arc::clone(&self.clock);
        let cq = self.cq.clone();
        self.handle.spawn(async move {
            let output = execute_tool(tools.as_ref(), &call, clock.as_ref(), events.as_ref()).await;
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

async fn execute_tool(
    tools: &ToolRegistry,
    call: &ToolCall,
    clock: &dyn EventClock,
    events: &dyn EventSink,
) -> tongs::tools::ToolOutput {
    let started_ms = clock.now_millis();
    let output = match tools.get(&call.name) {
        Some(tool) => match tool.execute(&call.id, call.arguments.clone(), None).await {
            Ok(output) => output,
            Err(error) => tool_error_output(&format!("tool `{}` failed: {error}", call.name)),
        },
        None => tool_error_output(&format!("unknown tool `{}`", call.name)),
    };
    let duration_ms = clock.now_millis().saturating_sub(started_ms);
    let status = if output.is_error {
        ToolCallStatus::Failed
    } else {
        ToolCallStatus::Succeeded
    };
    events.emit(AgentEvent::ToolEnd {
        id: call.id.clone(),
        name: call.name.clone(),
        status,
        duration_ms,
        result: bounded_tool_result(&output),
    });
    output
}

const TOOL_RESULT_PREVIEW_BYTES: usize = 4 * 1024;

/// Extract a bounded text-only candidate from a tool result. Structured details,
/// signatures, images, and arbitrary JSON never enter the event protocol.
fn bounded_tool_result(output: &tongs::tools::ToolOutput) -> ToolResultMetadata {
    let text = output
        .content
        .iter()
        .filter_map(|block| match block {
            tongs::model::ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let bytes = u64::try_from(text.len()).unwrap_or(u64::MAX);
    if text.is_empty() {
        return ToolResultMetadata {
            preview: None,
            bytes,
            truncated: false,
        };
    }
    let (preview, truncated) = truncate_utf8(&text, TOOL_RESULT_PREVIEW_BYTES);
    ToolResultMetadata {
        preview: Some(preview.to_string()),
        bytes,
        truncated,
    }
}

fn truncate_utf8(value: &str, maximum_bytes: usize) -> (&str, bool) {
    if value.len() <= maximum_bytes {
        return (value, false);
    }
    let mut end = maximum_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    (&value[..end], true)
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

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use tongs::tools::{Tool, ToolEffects, ToolOutput, ToolUpdate};

    struct FakeClock(Mutex<VecDeque<u64>>);

    impl EventClock for FakeClock {
        fn now_millis(&self) -> u64 {
            self.0
                .lock()
                .expect("clock")
                .pop_front()
                .expect("clock value")
        }
    }

    #[derive(Default)]
    struct Recorder(Mutex<Vec<AgentEvent>>);

    impl EventSink for Recorder {
        fn emit(&self, event: AgentEvent) {
            self.0.lock().expect("events").push(event);
        }
    }

    struct FakeTool;

    #[async_trait]
    impl Tool for FakeTool {
        fn name(&self) -> &str {
            "fake"
        }

        fn label(&self) -> &str {
            "fake"
        }

        fn description(&self) -> &str {
            "deterministic fake tool"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        fn effects(&self) -> ToolEffects {
            ToolEffects::read()
        }

        async fn execute(
            &self,
            _tool_call_id: &str,
            _input: serde_json::Value,
            _on_update: Option<Box<dyn Fn(ToolUpdate) + Send + Sync>>,
        ) -> tongs::Result<ToolOutput> {
            Ok(ToolOutput {
                content: vec![tongs::model::ContentBlock::Text(
                    tongs::model::TextContent {
                        text: "bounded result".to_string(),
                        text_signature: None,
                    },
                )],
                details: None,
                is_error: false,
            })
        }
    }

    #[test]
    fn tool_duration_uses_the_injected_monotonic_clock() {
        let tools = ToolRegistry::from_tools(vec![Box::new(FakeTool)]);
        let clock = FakeClock(Mutex::new(VecDeque::from([100, 137])));
        let recorder = Arc::new(Recorder::default());
        let observed = Arc::clone(&recorder);
        let call = ToolCall {
            id: "call-1".to_string(),
            name: "fake".to_string(),
            arguments: serde_json::json!({}),
        };

        let output = temper_agent_io::block_on(async move {
            execute_tool(&tools, &call, &clock, observed.as_ref()).await
        });
        assert!(!output.is_error);
        let events = recorder.0.lock().expect("events");
        assert!(matches!(
            &events[0],
            AgentEvent::ToolEnd {
                id,
                name,
                status: ToolCallStatus::Succeeded,
                duration_ms: 37,
                result: ToolResultMetadata {
                    preview: Some(preview),
                    bytes: 14,
                    truncated: false,
                },
            } if id == "call-1" && name == "fake" && preview == "bounded result"
        ));
    }

    #[test]
    fn bounded_tool_metadata_is_utf8_safe_and_omits_structured_details() {
        let output = tongs::tools::ToolOutput {
            content: vec![tongs::model::ContentBlock::Text(
                tongs::model::TextContent {
                    text: "🙂".repeat(2_000),
                    text_signature: None,
                },
            )],
            details: Some(serde_json::json!({"secret": "must-not-enter-preview"})),
            is_error: false,
        };
        let metadata = bounded_tool_result(&output);
        assert!(metadata.truncated);
        assert!(
            metadata
                .preview
                .as_ref()
                .is_some_and(|value| value.len() <= TOOL_RESULT_PREVIEW_BYTES)
        );
        assert!(
            !metadata
                .preview
                .as_deref()
                .unwrap_or_default()
                .contains("must-not-enter-preview")
        );
    }
}
