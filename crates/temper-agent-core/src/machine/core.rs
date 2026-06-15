//! The driving logic of the pure agent loop: [`AgentMachine`] and its
//! `(state, completion) -> [request]` step function.
//!
//! This is the heart of the sans-IO loop — when to call the model, which tool
//! batch to dispatch, when to inject steering, and when to stop. The protocol
//! types it exchanges live in [`super::protocol`]; the effect-batching policy
//! it applies lives in [`super::batching`].

use std::collections::{BTreeMap, VecDeque};

use tongs::model::{
    AssistantMessage, ContentBlock, Message, StopReason, ToolCall, ToolResultMessage,
};
use tongs::tools::{ToolEffects, ToolOutput};

use super::batching::{PendingTool, plan_batches};
use super::protocol::{AgentCompletion, AgentEvent, AgentRequest, AgentStop};

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
    pending_batches: VecDeque<Vec<PendingTool>>,
    /// Results collected this turn across all batches, in original tool-call
    /// order, so the tool-result messages are appended deterministically.
    turn_results: Vec<PendingTool>,
    /// The most recent assistant message (the run's product on completion).
    last_assistant: Option<AssistantMessage>,
    /// Steering messages to inject at the next turn boundary.
    queued_steering: Vec<Message>,
    aborted: bool,
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
            pending_batches: VecDeque::new(),
            turn_results: Vec::new(),
            last_assistant: None,
            queued_steering: Vec::new(),
            aborted: false,
        }
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
        requests.push(AgentRequest::Emit(AgentEvent::TurnStart {
            turn: self.turn,
        }));
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
        self.pending_batches = plan_batches(&self.effects, &tool_calls);
        requests.extend(self.dispatch_current_batch());
        requests
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

impl temper_agent_io::Machine for AgentMachine {
    type Completion = AgentCompletion;
    type Request = AgentRequest;

    fn on_start(&mut self, _now: temper_agent_io::EngineTime) -> Vec<AgentRequest> {
        self.begin_turn()
    }

    fn on_completion(
        &mut self,
        _now: temper_agent_io::EngineTime,
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
        content: vec![ContentBlock::Text(tongs::model::TextContent {
            text: message.to_string(),
            text_signature: None,
        })],
        api: String::new(),
        provider: String::new(),
        model: String::new(),
        usage: tongs::model::Usage::default(),
        stop_reason: StopReason::Error,
        error_message: Some(message.to_string()),
        timestamp: 0,
    }
}
