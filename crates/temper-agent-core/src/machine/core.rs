//! The driving logic of the pure agent loop: [`AgentMachine`] and its
//! `(state, completion) -> [request]` step function.
//!
//! This is the heart of the sans-IO loop — when to call the model, which tool
//! batch to dispatch, when to inject steering, and when to stop. The protocol
//! types it exchanges live in [`super::protocol`]; the effect-batching policy
//! it applies lives in [`super::batching`].

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use tongs::model::{
    AssistantMessage, ContentBlock, Message, StopReason, ToolCall, ToolResultMessage,
};
use tongs::tools::{ToolEffects, ToolOutput};

/// Computes the one-line salient-argument preview shown in `ToolStart`
/// observability events. Supplied by the shell-side caller (it lives above this
/// tier, where the workspace `cwd` and per-tool rendering rules are known); the
/// pure core just calls it with each call's name + parsed arguments. See the
/// agent-log-cleanup plan (pieces B/D).
pub type ArgPreviewFn = Arc<dyn Fn(&str, &serde_json::Value) -> Option<String> + Send + Sync>;

use crate::model_failure::ModelFailureDiagnostic;

use super::batching::{PendingTool, plan_batches};
use super::protocol::{
    AgentCompletion, AgentEvent, AgentRequest, AgentStop, BatchGeneration, OperationGeneration,
};

/// Where the loop is in the call/tool cycle.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Phase {
    /// Waiting for a model response.
    AwaitingLlm,
    /// Waiting for the in-flight tool batch to finish.
    AwaitingTools,
    /// Cancellation has been requested; only a matching shell-quiescence
    /// completion can finish the run.
    Cancelling,
    /// Terminal.
    Done,
}

#[derive(Debug)]
struct ActiveToolBatch {
    generation: BatchGeneration,
    operations: BTreeMap<String, OperationGeneration>,
    settled: BTreeSet<String>,
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
    /// Structured terminal provider/model failure, kept independently from
    /// the compatibility assistant message.
    model_failure: Option<ModelFailureDiagnostic>,
    /// Steering messages to inject at the next turn boundary.
    queued_steering: Vec<Message>,
    /// Next never-reused shell operation identity.
    next_operation_generation: OperationGeneration,
    /// Next never-reused parallel tool-batch identity. Model calls use zero.
    next_batch_generation: BatchGeneration,
    /// Model operation currently allowed to settle.
    active_llm: Option<OperationGeneration>,
    /// Tool batch currently allowed to settle, including duplicate detection.
    active_tool_batch: Option<ActiveToolBatch>,
    /// Fresh operation/batch pair attached to the outstanding cancellation.
    cancellation_generation: Option<(OperationGeneration, BatchGeneration)>,
    /// Optional shell-supplied preview function used to fill
    /// `ToolStart.arg_preview` from each call's name + arguments. `None` leaves
    /// the field unset (the pure default).
    arg_preview: Option<ArgPreviewFn>,
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
            model_failure: None,
            queued_steering: Vec::new(),
            next_operation_generation: 1,
            next_batch_generation: 1,
            active_llm: None,
            active_tool_batch: None,
            cancellation_generation: None,
            arg_preview: None,
        }
    }

    /// Installs the shell-supplied [`ArgPreviewFn`] used to fill
    /// `ToolStart.arg_preview`. Without it the field stays `None`.
    pub fn with_arg_preview(mut self, arg_preview: ArgPreviewFn) -> Self {
        self.arg_preview = Some(arg_preview);
        self
    }

    /// The current conversation (test/observability accessor).
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    fn finish(&mut self, stop: AgentStop) -> Vec<AgentRequest> {
        self.phase = Phase::Done;
        self.active_llm = None;
        self.active_tool_batch = None;
        self.pending_batches.clear();
        self.cancellation_generation = None;
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
                model_failure: self.model_failure.take(),
            },
        ]
    }

    fn next_operation_generation(&mut self) -> OperationGeneration {
        let generation = self.next_operation_generation;
        self.next_operation_generation = self
            .next_operation_generation
            .checked_add(1)
            .expect("agent operation generation exhausted");
        generation
    }

    fn next_batch_generation(&mut self) -> BatchGeneration {
        let generation = self.next_batch_generation;
        self.next_batch_generation = self
            .next_batch_generation
            .checked_add(1)
            .expect("agent batch generation exhausted");
        generation
    }

    /// The operation/batch identity currently allowed to complete. This is
    /// primarily useful to deterministic protocol tests that synthesize shell
    /// completions without running an executor.
    pub fn active_generations(&self) -> Option<(OperationGeneration, BatchGeneration)> {
        match self.phase {
            Phase::AwaitingLlm => self.active_llm.map(|operation| (operation, 0)),
            Phase::AwaitingTools => self.active_tool_batch.as_ref().and_then(|batch| {
                batch
                    .operations
                    .values()
                    .next()
                    .copied()
                    .map(|operation| (operation, batch.generation))
            }),
            Phase::Cancelling => self.cancellation_generation,
            Phase::Done => None,
        }
    }

    /// The operation/batch identity currently allowed to complete for `id`.
    /// Returns `None` unless that exact tool call is in the active batch.
    pub fn active_tool_generations(
        &self,
        id: &str,
    ) -> Option<(OperationGeneration, BatchGeneration)> {
        let batch = self.active_tool_batch.as_ref()?;
        batch
            .operations
            .get(id)
            .copied()
            .map(|operation| (operation, batch.generation))
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
        let operation_generation = self.next_operation_generation();
        self.active_llm = Some(operation_generation);
        self.active_tool_batch = None;
        requests.push(AgentRequest::Emit(AgentEvent::TurnStart {
            turn: self.turn,
        }));
        requests.push(AgentRequest::CallLlm {
            operation_generation,
            batch_generation: 0,
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
        let calls = batch
            .iter()
            .map(|pending| pending.call.clone())
            .collect::<Vec<_>>();
        let batch_generation = self.next_batch_generation();
        let mut operations = BTreeMap::new();
        let mut requests = Vec::new();
        for call in calls {
            // The pure core does not know the per-tool rendering rules; the
            // shell supplies an optional preview fn (agent-log-cleanup plan,
            // pieces B/D). Absent it, the field stays `None`.
            let arg_preview = self
                .arg_preview
                .as_ref()
                .and_then(|render| render(&call.name, &call.arguments));
            let operation_generation = self.next_operation_generation();
            operations.insert(call.id.clone(), operation_generation);
            requests.push(AgentRequest::Emit(AgentEvent::ToolStart {
                id: call.id.clone(),
                name: call.name.clone(),
                arg_preview,
            }));
            requests.push(AgentRequest::RunTool {
                operation_generation,
                batch_generation,
                call,
            });
        }
        self.active_llm = None;
        self.active_tool_batch = Some(ActiveToolBatch {
            generation: batch_generation,
            operations,
            settled: BTreeSet::new(),
        });
        requests
    }

    fn on_tool_finished(
        &mut self,
        operation_generation: OperationGeneration,
        batch_generation: BatchGeneration,
        id: String,
        output: ToolOutput,
    ) -> Vec<AgentRequest> {
        // A completion is accepted exactly once and only for the active batch.
        // This fences duplicated calls and late tasks from cancelled or prior
        // model turns before they can mutate the conversation.
        let Some(active) = self.active_tool_batch.as_mut() else {
            return Vec::new();
        };
        if !matches!(self.phase, Phase::AwaitingTools)
            || active.generation != batch_generation
            || active.operations.get(&id) != Some(&operation_generation)
            || !active.settled.insert(id.clone())
        {
            return Vec::new();
        }

        // The shell emits the timed ToolEnd event immediately before enqueueing
        // this completion. The pure machine only sequences the result into the
        // conversation, avoiding a second parallel instrumentation path.
        let mut requests = Vec::new();

        // Record the result into the in-flight (front) batch.
        if let Some(batch) = self.pending_batches.front_mut() {
            if let Some(pending) = batch.iter_mut().find(|p| p.call.id == id) {
                let tool_name = pending.call.name.clone();
                pending.result = Some(tool_result_message(&id, &tool_name, output));
            }
        }

        let batch_done = active.settled.len() == active.operations.len();
        if !batch_done {
            return requests;
        }
        self.active_tool_batch = None;

        // Retire the batch: its results join the turn's results in original
        // tool-call order (batches were planned in order, so appending preserves
        // it). Then run the next batch, or finish the turn.
        if let Some(batch) = self.pending_batches.pop_front() {
            self.turn_results.extend(batch);
        }

        if !self.pending_batches.is_empty() {
            requests.extend(self.dispatch_current_batch());
            return requests;
        }

        // All batches done: append every tool-result message in order, then
        // begin the next model turn.
        for pending in std::mem::take(&mut self.turn_results) {
            if let Some(result) = pending.result {
                self.messages
                    .push(Message::ToolResult(std::sync::Arc::new(result)));
            }
        }
        requests.extend(self.begin_turn());
        requests
    }

    fn begin_cancellation(&mut self) -> Vec<AgentRequest> {
        if matches!(self.phase, Phase::Done | Phase::Cancelling) {
            return Vec::new();
        }
        let batch_generation = self
            .active_tool_batch
            .as_ref()
            .map_or(0, |batch| batch.generation);
        let operation_generation = self.next_operation_generation();
        self.phase = Phase::Cancelling;
        self.active_llm = None;
        self.active_tool_batch = None;
        self.pending_batches.clear();
        self.turn_results.clear();
        self.queued_steering.clear();
        self.cancellation_generation = Some((operation_generation, batch_generation));
        vec![AgentRequest::CancelActive {
            operation_generation,
            batch_generation,
        }]
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
            AgentCompletion::LlmResponded {
                operation_generation,
                batch_generation,
                message,
            } => {
                if !matches!(self.phase, Phase::AwaitingLlm)
                    || batch_generation != 0
                    || self.active_llm != Some(operation_generation)
                {
                    return Vec::new();
                }
                self.active_llm = None;
                self.on_llm_responded(message)
            }
            AgentCompletion::LlmFailed {
                operation_generation,
                batch_generation,
                diagnostic,
            } => {
                if !matches!(self.phase, Phase::AwaitingLlm)
                    || batch_generation != 0
                    || self.active_llm != Some(operation_generation)
                {
                    return Vec::new();
                }
                self.active_llm = None;
                self.last_assistant = Some(error_assistant(diagnostic.message()));
                self.model_failure = Some(diagnostic);
                self.finish(AgentStop::ModelError)
            }
            AgentCompletion::ToolFinished {
                operation_generation,
                batch_generation,
                id,
                output,
            } => self.on_tool_finished(operation_generation, batch_generation, id, output),
            AgentCompletion::TasksQuiesced {
                operation_generation,
                batch_generation,
            } => {
                if !matches!(self.phase, Phase::Cancelling)
                    || self.cancellation_generation
                        != Some((operation_generation, batch_generation))
                {
                    return Vec::new();
                }
                self.finish(AgentStop::Aborted)
            }
            AgentCompletion::Steer(messages) => {
                if matches!(self.phase, Phase::Done | Phase::Cancelling) {
                    return Vec::new();
                }
                // Queue for the next turn boundary. If we are idle between turns
                // (shouldn't normally happen — the shell only delivers steering
                // while a run is active), it will be picked up on begin_turn.
                self.queued_steering.extend(messages);
                Vec::new()
            }
            AgentCompletion::Abort => self.begin_cancellation(),
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
