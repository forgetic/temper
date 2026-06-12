//! The pure agent loop, as a sans-IO state machine.
//!
//! [`AgentMachine`] is the functional core of an LLM agent turn: it owns the
//! conversation state and the iteration budget, and decides — purely — when to
//! call the model, which tools to run, when to inject steering, and when to
//! stop. It performs no I/O. The actual model streaming and tool execution are
//! done by the shell ([`crate::shell`]), which reuses pi-SDK providers and
//! tools and feeds results back as [`AgentCompletion`]s.
//!
//! This mirrors the [`smith_io_engine::Machine`] discipline used by the worker:
//! `(state, completion) -> [request]`, deterministic and replayable, so the
//! whole loop — tool orchestration, max-iteration cutoff, stop-reason handling,
//! steering at turn boundaries — is unit-testable with synthetic completions and
//! drivable under the asupersync lab for simulation/fuzz testing.
//!
//! Design note (steering): steering messages are injected at **turn
//! boundaries** — after a model turn and its tool batch complete, before the
//! next model call — not mid-tool-batch. This keeps the machine simple while
//! still supporting live interaction (the user's stated control goal); pi's
//! finer-grained mid-batch steering is deliberately not reproduced.

use std::collections::BTreeMap;

use pi::model::{
    AssistantMessage, ContentBlock, Message, StopReason, ToolCall, ToolResultMessage,
};
use pi::tools::{ToolEffects, ToolOutput};

/// An observability event the machine emits as data (the shell renders/records
/// it). Keeping events as machine output — rather than callbacks fired from
/// inside the loop, as pi does — preserves purity and makes the event stream
/// itself testable.
#[derive(Clone, Debug)]
pub enum AgentEvent {
    /// A model turn is starting (about to call the LLM).
    TurnStart { turn: usize },
    /// A live streaming delta from the model, emitted by the shell as the
    /// response streams in (before the turn's full [`AgentEvent::AssistantMessage`]).
    /// Lets observers — a TUI, a transcript recorder — watch tokens and tool
    /// calls arrive in real time. The machine never sees these; they are the
    /// shell's responsibility, so the loop stays pure.
    StreamDelta(StreamDelta),
    /// The model produced an assistant message.
    AssistantMessage { content: Vec<ContentBlock> },
    /// A tool is about to run.
    ToolStart { id: String, name: String },
    /// A tool finished.
    ToolEnd { id: String, is_error: bool },
    /// Steering messages were injected at a turn boundary.
    Steered { count: usize },
    /// The agent run ended (with the reason it stopped).
    AgentEnd { reason: AgentStop },
}

/// A live streaming fragment of a model response, forwarded by the shell.
#[derive(Clone, Debug)]
pub enum StreamDelta {
    /// A chunk of assistant text.
    Text(String),
    /// A chunk of model "thinking" (extended reasoning).
    Thinking(String),
    /// A tool call finished streaming (its arguments are now complete).
    ToolCall { id: String, name: String },
}

/// Why the agent loop stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentStop {
    /// The model returned a non-tool-use stop (normal completion).
    Completed,
    /// The model signalled an error stop reason.
    ModelError,
    /// The run was aborted (cancellation/steering-to-stop).
    Aborted,
    /// The tool-iteration budget was exhausted.
    BudgetExhausted,
}

/// A finished I/O event delivered to the machine.
pub enum AgentCompletion {
    /// The model stream completed, yielding the full assistant message.
    LlmResponded(AssistantMessage),
    /// The model call failed at the transport/provider layer.
    LlmFailed(String),
    /// A tool the machine requested finished.
    ToolFinished { id: String, output: ToolOutput },
    /// Steering messages arrived from the controller; inject at the next turn
    /// boundary.
    Steer(Vec<Message>),
    /// The run was asked to abort.
    Abort,
}

/// An I/O request the shell must perform.
pub enum AgentRequest {
    /// Stream a model response over the current message history. The shell
    /// builds the provider `Context` from these messages + the agent's system
    /// prompt and tool defs (held by the shell), and replies with
    /// [`AgentCompletion::LlmResponded`] / [`AgentCompletion::LlmFailed`].
    CallLlm { messages: Vec<Message> },
    /// Run one tool call; reply with [`AgentCompletion::ToolFinished`].
    RunTool(ToolCall),
    /// Emit an observability event.
    Emit(AgentEvent),
    /// The run is finished; `final_message` is the last assistant message (or a
    /// synthesized terminal message). The shell resolves the run with it.
    Finished {
        stop: AgentStop,
        final_message: AssistantMessage,
        messages: Vec<Message>,
    },
}

/// Where the loop is in the call/tool cycle.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Phase {
    /// Waiting for a model response.
    AwaitingLlm,
    /// Waiting for the in-flight tool batch to finish.
    AwaitingTools,
    /// Terminal.
    Done,
}

/// The pure agent loop.
pub struct AgentMachine {
    messages: Vec<Message>,
    max_iterations: usize,
    iterations: usize,
    phase: Phase,
    turn: usize,
    /// Per-tool-name effect declarations, used to plan effect-compatible
    /// parallel batches. Unknown tools default to a write effect (fail-closed:
    /// serialize). Static config, supplied at construction by the shell.
    effects: BTreeMap<String, ToolEffects>,
    /// Effect-compatible batches still to run this turn, in original tool-call
    /// order (front = the batch currently in flight). Each batch's calls run
    /// concurrently; batches run strictly in sequence.
    pending_batches: std::collections::VecDeque<Vec<PendingTool>>,
    /// Results collected this turn across all batches, in original tool-call
    /// order, so the tool-result messages are appended deterministically.
    turn_results: Vec<PendingTool>,
    /// The most recent assistant message (the run's product on completion).
    last_assistant: Option<AssistantMessage>,
    /// Steering messages to inject at the next turn boundary.
    queued_steering: Vec<Message>,
    aborted: bool,
}

struct PendingTool {
    call: ToolCall,
    result: Option<ToolResultMessage>,
}

impl AgentMachine {
    /// Build a machine seeded with the initial conversation (typically a single
    /// user message), bounded to `max_iterations` tool rounds. Tools run
    /// serialized (every tool is treated as a write) — use [`AgentMachine::with_effects`]
    /// to supply effect declarations and enable parallel batching.
    pub fn new(initial_messages: Vec<Message>, max_iterations: usize) -> Self {
        Self::with_effects(initial_messages, max_iterations, BTreeMap::new())
    }

    /// Build a machine that plans effect-compatible parallel tool batches from
    /// `effects` (tool name → its [`ToolEffects`]). Adjacent calls whose effects
    /// are mutually parallel-safe (read-only) run concurrently; a write/network/
    /// process tool — or an unknown tool, fail-closed — forms a serialized
    /// batch boundary, mirroring pi's tool-effect batching policy.
    pub fn with_effects(
        initial_messages: Vec<Message>,
        max_iterations: usize,
        effects: BTreeMap<String, ToolEffects>,
    ) -> Self {
        Self {
            messages: initial_messages,
            max_iterations,
            iterations: 0,
            phase: Phase::AwaitingLlm,
            turn: 0,
            effects,
            pending_batches: std::collections::VecDeque::new(),
            turn_results: Vec::new(),
            last_assistant: None,
            queued_steering: Vec::new(),
            aborted: false,
        }
    }

    /// The effect declaration for a tool name, defaulting to write (serialize)
    /// for unknown tools — fail-closed, matching pi.
    fn effects_for(&self, name: &str) -> ToolEffects {
        self.effects.get(name).copied().unwrap_or_else(ToolEffects::write)
    }

    /// The current conversation (test/observability accessor).
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    fn finish(&mut self, stop: AgentStop) -> Vec<AgentRequest> {
        self.phase = Phase::Done;
        let final_message = self
            .last_assistant
            .clone()
            .unwrap_or_else(|| error_assistant("agent ended before producing a message"));
        vec![
            AgentRequest::Emit(AgentEvent::AgentEnd { reason: stop }),
            AgentRequest::Finished {
                stop,
                final_message,
                messages: std::mem::take(&mut self.messages),
            },
        ]
    }

    /// Begin the next model turn: inject any queued steering, then call the LLM.
    fn begin_turn(&mut self) -> Vec<AgentRequest> {
        let mut requests = Vec::new();
        if !self.queued_steering.is_empty() {
            let steering = std::mem::take(&mut self.queued_steering);
            requests.push(AgentRequest::Emit(AgentEvent::Steered {
                count: steering.len(),
            }));
            self.messages.extend(steering);
        }
        self.phase = Phase::AwaitingLlm;
        requests.push(AgentRequest::Emit(AgentEvent::TurnStart { turn: self.turn }));
        requests.push(AgentRequest::CallLlm {
            messages: self.messages.clone(),
        });
        self.turn += 1;
        requests
    }

    fn on_llm_responded(&mut self, assistant: AssistantMessage) -> Vec<AgentRequest> {
        let mut requests = vec![AgentRequest::Emit(AgentEvent::AssistantMessage {
            content: assistant.content.clone(),
        })];
        self.messages
            .push(Message::Assistant(std::sync::Arc::new(assistant.clone())));
        self.last_assistant = Some(assistant.clone());

        if matches!(assistant.stop_reason, StopReason::Error) {
            requests.extend(self.finish(AgentStop::ModelError));
            return requests;
        }
        if matches!(assistant.stop_reason, StopReason::Aborted) {
            requests.extend(self.finish(AgentStop::Aborted));
            return requests;
        }

        let tool_calls = extract_tool_calls(&assistant.content);
        if tool_calls.is_empty() {
            // No tools requested ⇒ the model is done.
            requests.extend(self.finish(AgentStop::Completed));
            return requests;
        }

        // Tool round: enforce the iteration budget before dispatching.
        self.iterations += 1;
        if self.iterations > self.max_iterations {
            requests.extend(self.finish(AgentStop::BudgetExhausted));
            return requests;
        }

        // Plan effect-compatible batches: adjacent parallel-safe calls run
        // together; a barrier (write/network/process/unknown) starts a new
        // serialized batch. This is pure policy over the calls' declared effects.
        self.phase = Phase::AwaitingTools;
        self.turn_results.clear();
        self.pending_batches = self.plan_batches(&tool_calls);
        requests.extend(self.dispatch_current_batch());
        requests
    }

    /// Partition tool calls into contiguous effect-compatible batches (front of
    /// the deque first). Mirrors pi's `plan_tool_effect_batches`: never reorders
    /// calls, groups adjacent mutually-parallel-safe effects, breaks at the
    /// first incompatible effect.
    fn plan_batches(&self, calls: &[ToolCall]) -> std::collections::VecDeque<Vec<PendingTool>> {
        let mut batches = std::collections::VecDeque::new();
        let mut current: Vec<PendingTool> = Vec::new();
        let mut active: Option<ToolEffects> = None;
        for call in calls {
            let call_effects = self.effects_for(&call.name);
            let compatible = match active {
                Some(active_effects) => active_effects.compatible_with(call_effects),
                None => true,
            };
            if compatible && !current.is_empty() {
                active = Some(active.expect("active set when current non-empty").union(call_effects));
                current.push(PendingTool {
                    call: call.clone(),
                    result: None,
                });
            } else {
                if !current.is_empty() {
                    batches.push_back(std::mem::take(&mut current));
                }
                active = Some(call_effects);
                current.push(PendingTool {
                    call: call.clone(),
                    result: None,
                });
            }
        }
        if !current.is_empty() {
            batches.push_back(current);
        }
        batches
    }

    /// Emit ToolStart + RunTool for every call in the front batch (they run
    /// concurrently in the shell). The batch's calls are moved into
    /// `turn_results` slots as they finish.
    fn dispatch_current_batch(&mut self) -> Vec<AgentRequest> {
        let Some(batch) = self.pending_batches.front() else {
            return Vec::new();
        };
        let mut requests = Vec::new();
        for pending in batch {
            requests.push(AgentRequest::Emit(AgentEvent::ToolStart {
                id: pending.call.id.clone(),
                name: pending.call.name.clone(),
            }));
            requests.push(AgentRequest::RunTool(pending.call.clone()));
        }
        requests
    }

    fn on_tool_finished(&mut self, id: String, output: ToolOutput) -> Vec<AgentRequest> {
        let mut requests = vec![AgentRequest::Emit(AgentEvent::ToolEnd {
            id: id.clone(),
            is_error: output.is_error,
        })];

        // Record the result into the in-flight (front) batch.
        if let Some(batch) = self.pending_batches.front_mut()
            && let Some(pending) = batch.iter_mut().find(|p| p.call.id == id)
        {
            let tool_name = pending.call.name.clone();
            pending.result = Some(tool_result_message(&id, &tool_name, output));
        }

        // Is the front batch fully resolved?
        let batch_done = self
            .pending_batches
            .front()
            .is_some_and(|batch| batch.iter().all(|p| p.result.is_some()));
        if !batch_done {
            return requests;
        }

        // Retire the batch: its results join the turn's results in original
        // tool-call order (batches were planned in order, so appending preserves
        // it). Then run the next batch, or finish the turn.
        if let Some(batch) = self.pending_batches.pop_front() {
            self.turn_results.extend(batch);
        }

        // If an abort arrived mid-turn, drain the in-flight batch (done above)
        // but do NOT start any further batches — stop after appending results.
        if !self.aborted && !self.pending_batches.is_empty() {
            // More serialized batches remain — dispatch the next one. The
            // in-flight batch always drains fully before the run reacts to
            // steering/abort, keeping tool-result state untorn.
            requests.extend(self.dispatch_current_batch());
            return requests;
        }

        // All batches done: append every tool-result message in order, then
        // begin the next model turn (or stop if aborted mid-turn).
        for pending in std::mem::take(&mut self.turn_results) {
            if let Some(result) = pending.result {
                self.messages
                    .push(Message::ToolResult(std::sync::Arc::new(result)));
            }
        }
        if self.aborted {
            requests.extend(self.finish(AgentStop::Aborted));
        } else {
            requests.extend(self.begin_turn());
        }
        requests
    }
}

impl smith_io_engine::Machine for AgentMachine {
    type Completion = AgentCompletion;
    type Request = AgentRequest;

    fn on_start(&mut self, _now: smith_io_engine::EngineTime) -> Vec<AgentRequest> {
        self.begin_turn()
    }

    fn on_completion(
        &mut self,
        _now: smith_io_engine::EngineTime,
        completion: AgentCompletion,
    ) -> Vec<AgentRequest> {
        match completion {
            AgentCompletion::LlmResponded(assistant) => self.on_llm_responded(assistant),
            AgentCompletion::LlmFailed(message) => {
                self.last_assistant = Some(error_assistant(&message));
                self.finish(AgentStop::ModelError)
            }
            AgentCompletion::ToolFinished { id, output } => self.on_tool_finished(id, output),
            AgentCompletion::Steer(messages) => {
                // Queue for the next turn boundary. If we are idle between turns
                // (shouldn't normally happen — the shell only delivers steering
                // while a run is active), it will be picked up on begin_turn.
                self.queued_steering.extend(messages);
                Vec::new()
            }
            AgentCompletion::Abort => {
                self.aborted = true;
                // If we're mid-LLM or between turns, stop now; if mid-tools, let
                // the in-flight batch drain (on_tool_finished checks `aborted`).
                if matches!(self.phase, Phase::AwaitingTools) {
                    Vec::new()
                } else {
                    self.finish(AgentStop::Aborted)
                }
            }
        }
    }

    fn is_stopped(&self) -> bool {
        matches!(self.phase, Phase::Done)
    }
}

/// Pulls the tool-call blocks out of an assistant message, in order.
fn extract_tool_calls(content: &[ContentBlock]) -> Vec<ToolCall> {
    content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolCall(call) => Some(call.clone()),
            _ => None,
        })
        .collect()
}

/// Builds the tool-result message appended to the conversation after a tool runs.
fn tool_result_message(
    tool_call_id: &str,
    tool_name: &str,
    output: ToolOutput,
) -> ToolResultMessage {
    ToolResultMessage {
        tool_call_id: tool_call_id.to_string(),
        tool_name: tool_name.to_string(),
        content: output.content,
        details: output.details,
        is_error: output.is_error,
        timestamp: 0,
    }
}

/// Synthesizes a terminal assistant message carrying an error string, for the
/// paths where the run ends without a real model message.
fn error_assistant(message: &str) -> AssistantMessage {
    AssistantMessage {
        content: vec![ContentBlock::Text(pi::model::TextContent {
            text: message.to_string(),
            text_signature: None,
        })],
        api: String::new(),
        provider: String::new(),
        model: String::new(),
        usage: pi::model::Usage::default(),
        stop_reason: StopReason::Error,
        error_message: Some(message.to_string()),
        timestamp: 0,
    }
}

#[cfg(test)]
#[path = "machine_tests.rs"]
mod tests;
