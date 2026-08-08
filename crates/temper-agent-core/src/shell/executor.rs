//! The agent shell's I/O orchestration: the public shell types and the
//! [`Executor`] that performs the loop's two I/O seams.
//!
//! [`AgentShell`] owns every model/tool task in a per-run group. Operations are
//! deadline-bound and cancellation drops their futures immediately; the pure
//! machine is told that the group is quiescent only after every wrapper task has
//! returned.

use std::sync::Arc;
use std::time::Duration;

use temper_agent_io::{CqSender, Executor};
use tongs::model::{AssistantMessage, Message, ToolCall};
use tongs::provider::{Provider, StreamOptions, ToolDef};
use tongs::tools::ToolRegistry;

use crate::machine::{
    AgentCompletion, AgentEvent, AgentMachine, AgentRequest, AgentStop, BatchGeneration,
    CODEBASE_MEMORY_TOOL_PREFIX, CodebaseMemoryTiming, OperationGeneration,
    SAFE_GRAPH_CORRELATION_DETAIL_KEY, SAFE_TOOL_FAILURE_DETAIL_KEY, ToolCallStatus,
    ToolFailureCategory, ToolFailureDiagnostic, ToolResultMetadata,
};
use crate::model_failure::ModelFailureDiagnostic;
use crate::run::AgentOperationLimits;
use crate::shell::streaming::{
    ModelCallObservability, ModelOperationContext, ModelTaskOutcome, SYSTEM_STREAM_RETRY_RUNTIME,
    stream_to_completion,
};
use crate::shell::task_group::{CancellationToken, RunTaskGroup, cancel_or};

/// The settled result of a sub-agent run.
#[derive(Clone, Debug)]
pub struct AgentOutcome {
    pub stop: AgentStop,
    pub final_message: AssistantMessage,
    pub messages: Vec<Message>,
    /// Authoritative terminal model failure. A synthetic final message may
    /// duplicate its safe message for compatibility, but is not authoritative.
    pub model_failure: Option<ModelFailureDiagnostic>,
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
    operation_limits: AgentOperationLimits,
    task_group: RunTaskGroup,
    turn_hook: Option<Arc<dyn TurnHook>>,
    turns_started: std::sync::atomic::AtomicUsize,
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
        operation_limits: AgentOperationLimits,
        observability: RunObservability,
        outcome: temper_agent_io::OneshotSender<AgentOutcome>,
    ) -> Self {
        let task_group = RunTaskGroup::new(cq.clone());
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
            operation_limits,
            task_group,
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

    pub(crate) fn task_group(&self) -> RunTaskGroup {
        self.task_group.clone()
    }

    /// Spawn the model call for one turn: await the turn hook, stream the
    /// response, emit per-turn usage, and enqueue the generation-tagged
    /// completion.
    fn execute_call_llm(
        &self,
        operation_generation: OperationGeneration,
        batch_generation: BatchGeneration,
        messages: Vec<Message>,
    ) {
        let provider = Arc::clone(&self.provider);
        let system_prompt = self.system_prompt.clone();
        let tool_defs = Arc::clone(&self.tool_defs);
        let stream_options = Arc::clone(&self.stream_options);
        let events = Arc::clone(&self.events);
        let model = self.model.clone();
        let clock = Arc::clone(&self.clock);
        let cq = self.cq.clone();
        let turn_hook = self.turn_hook.clone();
        let limits = self.operation_limits;
        let (cancellation, task_guard) = self.task_group.register();
        let turn = self
            .turns_started
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.handle.spawn(async move {
            let _task_guard = task_guard;
            if let Some(hook) = turn_hook {
                if cancel_or(&cancellation, hook.before_model_call(turn))
                    .await
                    .is_none()
                {
                    return;
                }
            }
            let outcome = stream_to_completion(
                provider.as_ref(),
                system_prompt.as_deref(),
                &messages,
                &tool_defs,
                &stream_options,
                ModelOperationContext {
                    connect_timeout: limits.model_connect_timeout,
                    idle_timeout: limits.model_idle_timeout,
                    retry: limits.model_retry,
                    retry_runtime: &SYSTEM_STREAM_RETRY_RUNTIME,
                    cancellation: &cancellation,
                },
                ModelCallObservability {
                    turn,
                    model: &model,
                    clock: clock.as_ref(),
                    events: events.as_ref(),
                },
            )
            .await;
            let completion = match outcome {
                ModelTaskOutcome::Responded(message) => {
                    events.emit(AgentEvent::TurnUsage {
                        turn,
                        usage: message.usage,
                    });
                    AgentCompletion::LlmResponded {
                        operation_generation,
                        batch_generation,
                        message,
                    }
                }
                ModelTaskOutcome::Failed(diagnostic) => AgentCompletion::LlmFailed {
                    operation_generation,
                    batch_generation,
                    diagnostic,
                },
                ModelTaskOutcome::Cancelled => return,
            };
            let _ = cq.send(completion);
        });
    }

    /// Spawn one deadline-bound tool execution, measuring it on the same
    /// monotonic clock used for model attempts.
    fn execute_run_tool(
        &self,
        operation_generation: OperationGeneration,
        batch_generation: BatchGeneration,
        call: ToolCall,
    ) {
        let tools = Arc::clone(&self.tools);
        let events = Arc::clone(&self.events);
        let clock = Arc::clone(&self.clock);
        let cq = self.cq.clone();
        let timeout = self.operation_limits.tool_timeout;
        let (cancellation, task_guard) = self.task_group.register();
        self.handle.spawn(async move {
            let _task_guard = task_guard;
            let Some(output) = execute_tool(
                tools.as_ref(),
                &call,
                timeout,
                &cancellation,
                clock.as_ref(),
                events.as_ref(),
            )
            .await
            else {
                return;
            };
            let _ = cq.send(AgentCompletion::ToolFinished {
                operation_generation,
                batch_generation,
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
        model_failure: Option<ModelFailureDiagnostic>,
    ) {
        if let Some(sender) = self.outcome.lock().expect("outcome lock").take() {
            sender.send(AgentOutcome {
                stop,
                final_message,
                messages,
                model_failure,
            });
        }
    }
}

impl Executor<AgentMachine> for AgentShell {
    fn execute(&self, request: AgentRequest) {
        match request {
            AgentRequest::CallLlm {
                operation_generation,
                batch_generation,
                messages,
            } => self.execute_call_llm(operation_generation, batch_generation, messages),
            AgentRequest::RunTool {
                operation_generation,
                batch_generation,
                call,
            } => self.execute_run_tool(operation_generation, batch_generation, call),
            AgentRequest::CancelActive {
                operation_generation,
                batch_generation,
            } => self
                .task_group
                .cancel_all(operation_generation, batch_generation),
            AgentRequest::Emit(event) => self.events.emit(event),
            AgentRequest::Finished {
                stop,
                final_message,
                messages,
                model_failure,
            } => self.execute_finished(stop, final_message, messages, model_failure),
        }
    }
}

enum ToolExecution {
    Finished(tongs::tools::ToolOutput),
    TimedOut(tongs::tools::ToolOutput),
}

async fn execute_tool(
    tools: &ToolRegistry,
    call: &ToolCall,
    timeout: Duration,
    cancellation: &CancellationToken,
    clock: &dyn EventClock,
    events: &dyn EventSink,
) -> Option<tongs::tools::ToolOutput> {
    let started_ms = clock.now_millis();
    let operation = async {
        match tools.get(&call.name) {
            Some(tool) => match temper_agent_io::timeout(
                timeout,
                tool.execute(&call.id, call.arguments.clone(), None),
            )
            .await
            {
                Ok(Ok(output)) => ToolExecution::Finished(output),
                Ok(Err(error)) => ToolExecution::Finished(tool_error_output(&format!(
                    "tool `{}` failed: {error}",
                    call.name
                ))),
                Err(_) => ToolExecution::TimedOut(tool_error_output(&format!(
                    "tool `{}` timed out after configured limit {}",
                    call.name,
                    format_duration(timeout)
                ))),
            },
            None => {
                ToolExecution::Finished(tool_error_output(&format!("unknown tool `{}`", call.name)))
            }
        }
    };

    let settled = cancel_or(cancellation, operation).await;
    let duration_ms = clock.now_millis().saturating_sub(started_ms);
    let (output, status, timed_out) = match settled {
        Some(ToolExecution::Finished(output)) => {
            let status = if output.is_error {
                ToolCallStatus::Failed
            } else {
                ToolCallStatus::Succeeded
            };
            (output, status, false)
        }
        Some(ToolExecution::TimedOut(output)) => (output, ToolCallStatus::Cancelled, true),
        None => {
            let output = tool_error_output(&format!("tool `{}` cancelled", call.name));
            events.emit(AgentEvent::ToolEnd {
                id: call.id.clone(),
                name: call.name.clone(),
                status: ToolCallStatus::Cancelled,
                duration_ms,
                result: bounded_tool_result(&call.name, &output),
            });
            return None;
        }
    };
    events.emit(AgentEvent::ToolEnd {
        id: call.id.clone(),
        name: call.name.clone(),
        status,
        duration_ms,
        result: {
            let mut result = bounded_tool_result(&call.name, &output);
            if timed_out && call.name.starts_with(CODEBASE_MEMORY_TOOL_PREFIX) {
                result.failure = Some(ToolFailureDiagnostic::codebase_memory(
                    ToolFailureCategory::Timeout,
                ));
            }
            result
        },
    });
    Some(output)
}

fn format_duration(duration: Duration) -> String {
    if duration.subsec_nanos() == 0 {
        format!("{}s", duration.as_secs())
    } else if duration.subsec_nanos() % 1_000_000 == 0 {
        format!("{}ms", duration.as_millis())
    } else {
        format!("{}ns", duration.as_nanos())
    }
}

const TOOL_RESULT_PREVIEW_BYTES: usize = 4 * 1024;

/// Extract a bounded text-only candidate from a tool result. Generic structured
/// details, signatures, images, and arbitrary JSON never enter the event
/// protocol. A codebase-memory wrapper may contribute only a stable category,
/// bounded numeric timing fields, and a closed argument fingerprint.
fn bounded_tool_result(name: &str, output: &tongs::tools::ToolOutput) -> ToolResultMetadata {
    let failure = safe_tool_failure(name, output);
    let codebase_memory_timing = codebase_memory_timing(name, output);
    let graph_correlation = graph_correlation(name, output);
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
            failure,
            codebase_memory_timing,
            graph_correlation,
        };
    }
    let (preview, truncated) = truncate_utf8(&text, TOOL_RESULT_PREVIEW_BYTES);
    ToolResultMetadata {
        preview: Some(preview.to_string()),
        bytes,
        truncated,
        failure,
        codebase_memory_timing,
        graph_correlation,
    }
}

fn graph_correlation(
    name: &str,
    output: &tongs::tools::ToolOutput,
) -> Option<temper_protocol_activity::GraphCorrelationV1> {
    if !name.starts_with(CODEBASE_MEMORY_TOOL_PREFIX) || output.is_error {
        return None;
    }
    let correlation: temper_protocol_activity::GraphCorrelationV1 = serde_json::from_value(
        output
            .details
            .as_ref()?
            .get(SAFE_GRAPH_CORRELATION_DETAIL_KEY)?
            .clone(),
    )
    .ok()?;
    (correlation.is_valid() && correlation.tool.public_name() == name).then_some(correlation)
}

fn codebase_memory_timing(
    name: &str,
    output: &tongs::tools::ToolOutput,
) -> Option<CodebaseMemoryTiming> {
    if !name.starts_with(CODEBASE_MEMORY_TOOL_PREFIX) {
        return None;
    }
    let timing = output.details.as_ref()?.get("timing")?;
    Some(CodebaseMemoryTiming {
        readiness_wait_ms: timing.get("readiness_wait_ms")?.as_u64()?,
        graph_execution_ms: timing.get("graph_execution_ms")?.as_u64()?,
    })
}

fn safe_tool_failure(
    name: &str,
    output: &tongs::tools::ToolOutput,
) -> Option<ToolFailureDiagnostic> {
    if !output.is_error || !name.starts_with(CODEBASE_MEMORY_TOOL_PREFIX) {
        return None;
    }
    let marker = output.details.as_ref()?.get(SAFE_TOOL_FAILURE_DETAIL_KEY)?;
    if marker.get("source").and_then(serde_json::Value::as_str) != Some("codebase_memory") {
        return None;
    }
    let category = marker
        .get("category")
        .and_then(serde_json::Value::as_str)
        .and_then(ToolFailureCategory::from_stable_str)?;
    Some(ToolFailureDiagnostic::codebase_memory(category))
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
#[path = "executor_tests.rs"]
mod tests;
