//! The driving logic of the pure agent loop: [`AgentMachine`] and its
//! `(state, completion) -> [request]` step function.
//!
//! This is the heart of the sans-IO loop — when to call the model, which tool
//! batch to dispatch, when to inject steering, and when to stop. The protocol
//! types it exchanges live in [`super::protocol`]; the effect-batching policy
//! it applies lives in [`super::batching`].

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use temper_protocol_activity::{
    DecisionAnchorLineageV1, GraphCorrelationToolV1, GraphCorrelationV1, GraphExplorationClosedV1,
    GraphRecoveryReferenceDispositionV1, ShellDiscoveryDispositionV1,
};
use tongs::model::{
    AssistantMessage, ContentBlock, Message, StopReason, ToolCall, UserContent, UserMessage,
};
use tongs::tools::{ToolEffects, ToolOutput};

/// Computes the separated human and diagnostic presentations shown in a
/// `ToolStart` observability event. Supplied by the shell-side caller, where
/// workspace rendering and secret policy are known; the pure core never
/// interprets the returned content.
pub type ToolStartPresentationFn =
    Arc<dyn Fn(&str, &serde_json::Value) -> ToolStartPresentation + Send + Sync>;

/// Compatibility name retained for the existing run-builder parameter.
pub type ArgPreviewFn = ToolStartPresentationFn;

use crate::model_failure::ModelFailureDiagnostic;
use crate::{LineageAdmissionHandle, ToolInvocationCatalog};

use super::batching::{PendingTool, plan_batches};
use super::decision_anchor::{
    DECISION_ANCHOR_CONVERGENCE_MESSAGE, DECISION_ANCHOR_PROVIDER_UNAVAILABLE_FALLBACK_MESSAGE,
    DECISION_ANCHOR_RECOVERY_MESSAGE, DecisionAnchorState, DecisionAnchorTransition,
    SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY,
};
use super::messages::{error_assistant, tool_result_message};
use super::ordinary_failure::OrdinaryFailureCircuit;
use super::protocol::{
    AgentCompletion, AgentEvent, AgentRequest, AgentStop, BatchGeneration,
    CODEBASE_MEMORY_TOOL_PREFIX, OperationGeneration, SAFE_GRAPH_CORRELATION_DETAIL_KEY,
    ToolCallDenial, ToolStartPresentation,
};
use super::tool_failure::ToolFailureDiagnostic;

mod accessors;

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
    /// Final registry-derived definitions, aliases, schemas, and effects.
    invocation_catalog: Arc<ToolInvocationCatalog>,
    /// Typed local failures for calls scrubbed by the invocation boundary.
    invocation_rejections: BTreeMap<String, ToolFailureDiagnostic>,
    /// Content-free traversal kinds whose required selector was unusable
    /// before invocation scrubbing.
    incomplete_graph_selectors: BTreeMap<String, GraphCorrelationToolV1>,
    /// Content-free public graph kinds retained when canonical invocation
    /// validation scrubbed their arguments and names.
    rejected_graph_tools: BTreeMap<String, GraphCorrelationToolV1>,
    /// Closed opaque-reference classifications captured before malformed
    /// arguments are scrubbed by the public invocation boundary.
    pub(super) recovery_reference_dispositions:
        BTreeMap<String, temper_protocol_activity::GraphRecoveryReferenceDispositionV1>,
    /// Bounded per-run ordinary-tool identities. This state contains only
    /// process-local digests and is never projected through the protocol.
    ordinary_failures: OrdinaryFailureCircuit,
    /// Effect-compatible batches still to run this turn, in original tool-call
    /// order (front = the batch currently in flight). Each batch's calls run
    /// concurrently; batches run strictly in sequence.
    pending_batches: VecDeque<Vec<PendingTool>>,
    /// Results collected this turn across all batches, in original tool-call
    /// order, so the tool-result messages are appended deterministically.
    turn_results: Vec<PendingTool>,
    /// Per-run graph guard enabled whenever codebase-memory tools are present,
    /// including read-only roles with no mutation authorization.
    pub(super) decision_anchors: Option<DecisionAnchorState>,
    /// Optional wrapper-owned run-local selector resolver. Its results are
    /// closed process-local policy values and never enter events or messages.
    pub(super) lineage_admission: Option<LineageAdmissionHandle>,
    /// Fixed convergence instruction queued once complete current-root evidence
    /// closes graph exploration.
    decision_anchor_complete: bool,
    /// Generic, privacy-safe recovery instruction queued by an unconsumable
    /// anchor. It is distinct from operator steering.
    decision_anchor_recovery: bool,
    /// Actionable, privacy-safe guidance for bounded missing-evidence recovery.
    decision_anchor_gap_recovery: Option<GraphExplorationClosedV1>,
    /// Per-result closed active-root classifications queued after tool results.
    decision_anchor_guidance: Vec<String>,
    /// The one selector-complete active-root continuation, emitted after all
    /// result-local candidate lists and lifecycle guidance.
    pub(super) decision_anchor_active_handoff: Option<String>,
    /// Stops the run after incomplete enabled evidence exhausts bounded recovery.
    decision_anchor_incomplete: bool,
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
    /// Optional shell-supplied presentation function used to fill the separate
    /// human preview and diagnostic argument candidate on `ToolStart`.
    tool_start_presentation: Option<ToolStartPresentationFn>,
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
        Self::with_invocation_catalog(
            initial_messages,
            max_iterations,
            Arc::new(ToolInvocationCatalog::permissive(effects)),
        )
    }

    /// Build the production machine around one finalized registry-derived
    /// invocation catalog.
    pub fn with_invocation_catalog(
        initial_messages: Vec<Message>,
        max_iterations: usize,
        invocation_catalog: Arc<ToolInvocationCatalog>,
    ) -> Self {
        let decision_anchors = DecisionAnchorState::from_effects(invocation_catalog.effects());
        Self {
            messages: initial_messages,
            max_iterations,
            iterations: 0,
            phase: Phase::AwaitingLlm,
            turn: 0,
            invocation_catalog,
            invocation_rejections: BTreeMap::new(),
            incomplete_graph_selectors: BTreeMap::new(),
            rejected_graph_tools: BTreeMap::new(),
            recovery_reference_dispositions: BTreeMap::new(),
            ordinary_failures: OrdinaryFailureCircuit::default(),
            pending_batches: VecDeque::new(),
            turn_results: Vec::new(),
            decision_anchors,
            lineage_admission: None,
            decision_anchor_complete: false,
            decision_anchor_recovery: false,
            decision_anchor_gap_recovery: None,
            decision_anchor_guidance: Vec::new(),
            decision_anchor_active_handoff: None,
            decision_anchor_incomplete: false,
            last_assistant: None,
            model_failure: None,
            queued_steering: Vec::new(),
            next_operation_generation: 1,
            next_batch_generation: 1,
            active_llm: None,
            active_tool_batch: None,
            cancellation_generation: None,
            tool_start_presentation: None,
        }
    }

    /// Installs the shell-supplied [`ArgPreviewFn`] used to finalize the
    /// separate human and diagnostic `ToolStart` presentations.
    pub fn with_arg_preview(mut self, arg_preview: ArgPreviewFn) -> Self {
        self.tool_start_presentation = Some(arg_preview);
        self
    }

    /// Installs the trusted run-local pre-provider lineage resolver.
    pub fn with_lineage_admission(mut self, admission: LineageAdmissionHandle) -> Self {
        self.lineage_admission = Some(admission);
        self
    }

    fn finish(&mut self, stop: AgentStop) -> Vec<AgentRequest> {
        self.phase = Phase::Done;
        self.active_llm = None;
        self.active_tool_batch = None;
        self.pending_batches.clear();
        self.cancellation_generation = None;
        self.decision_anchor_complete = false;
        self.decision_anchor_recovery = false;
        self.decision_anchor_gap_recovery = None;
        self.decision_anchor_guidance.clear();
        self.decision_anchor_active_handoff = None;
        self.decision_anchor_incomplete = false;
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
        for guidance in std::mem::take(&mut self.decision_anchor_guidance) {
            self.messages.push(Message::User(UserMessage {
                content: UserContent::Text(guidance),
                timestamp: 0,
            }));
        }
        if self.decision_anchor_complete {
            self.decision_anchor_complete = false;
            self.messages.push(Message::User(UserMessage {
                content: UserContent::Text(DECISION_ANCHOR_CONVERGENCE_MESSAGE.to_string()),
                timestamp: 0,
            }));
        }
        if self.decision_anchor_recovery {
            self.decision_anchor_recovery = false;
            self.messages.push(Message::User(UserMessage {
                content: UserContent::Text(DECISION_ANCHOR_RECOVERY_MESSAGE.to_string()),
                timestamp: 0,
            }));
        }
        if let Some(details) = self.decision_anchor_gap_recovery.take() {
            self.messages.push(Message::User(UserMessage {
                content: UserContent::Text(details.model_message()),
                timestamp: 0,
            }));
        }
        if let Some(guidance) = self.decision_anchor_active_handoff.take() {
            self.messages.push(Message::User(UserMessage {
                content: UserContent::Text(guidance),
                timestamp: 0,
            }));
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

    fn on_llm_responded(&mut self, mut assistant: AssistantMessage) -> Vec<AgentRequest> {
        self.capture_recovery_reference_dispositions(&assistant);
        // Normalize before the assistant turn is emitted, retained, inspected
        // by policy, previewed, batched, or dispatched.
        (
            self.invocation_rejections,
            self.incomplete_graph_selectors,
            self.rejected_graph_tools,
        ) = self.invocation_catalog.canonicalize_message(&mut assistant);
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
        self.pending_batches = plan_batches(self.invocation_catalog.effects(), &tool_calls);
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
        let model_turn = self.turn.saturating_sub(1);
        let active_decision_root = self
            .decision_anchors
            .as_ref()
            .and_then(|state| state.active_root_binding())
            .map(str::to_string);
        let resolved_admissions = calls
            .iter()
            .map(|call| {
                (!self.invocation_rejections.contains_key(&call.id)
                    && call.name.starts_with(CODEBASE_MEMORY_TOOL_PREFIX))
                .then(|| {
                    self.lineage_admission.as_ref().map(|admission| {
                        admission.resolve_for_active_root_with_recovery(
                            &call.name,
                            &call.arguments,
                            active_decision_root.as_deref(),
                        )
                    })
                })
                .flatten()
            })
            .collect::<Vec<_>>();
        let closed_admissions = resolved_admissions
            .iter()
            .map(|resolved| resolved.as_ref().map(|(admission, _)| admission.clone()))
            .collect::<Vec<_>>();
        let incomplete_graph_selectors = calls
            .iter()
            .map(|call| self.incomplete_graph_selectors.get(&call.id).copied())
            .collect::<Vec<_>>();
        let recovery_reference_dispositions = self.resolve_recovery_reference_dispositions(
            &calls,
            &resolved_admissions,
            &incomplete_graph_selectors,
            active_decision_root.as_deref(),
        );
        let invocation_targets = calls
            .iter()
            .map(|call| {
                (!self.invocation_rejections.contains_key(&call.id))
                    .then(|| {
                        self.lineage_admission.as_ref().map(|admission| {
                            admission.resolve_invocation_targets(&call.name, &call.arguments)
                        })
                    })
                    .flatten()
            })
            .collect::<Vec<_>>();
        if let Some(batch) = self.pending_batches.front_mut() {
            for (pending, admission) in batch.iter_mut().zip(invocation_targets.iter().cloned()) {
                pending.invocation_targets = admission;
            }
        }
        let mut active_handoff = None;
        let denials = if let Some(state) = self.decision_anchors.as_mut() {
            let denials = state.on_tool_batch_dispatched_with_closed_inputs(
                &calls,
                model_turn,
                &closed_admissions,
                &invocation_targets,
                &incomplete_graph_selectors,
                &recovery_reference_dispositions,
            );
            let local_graph_correction = calls
                .iter()
                .zip(&denials)
                .zip(&recovery_reference_dispositions)
                .any(|((call, denial), disposition)| {
                    let graph_call = call.name.starts_with(CODEBASE_MEMORY_TOOL_PREFIX)
                        || self.rejected_graph_tools.contains_key(&call.id)
                        || self.incomplete_graph_selectors.contains_key(&call.id);
                    graph_call
                        && (denial.is_some()
                            || self.invocation_rejections.contains_key(&call.id)
                            || *disposition == Some(GraphRecoveryReferenceDispositionV1::Rejected))
                });
            let candidate_handoff = state.active_recovery_action();
            let guidance = state.take_model_guidance();
            if !guidance.is_empty() || local_graph_correction {
                active_handoff = candidate_handoff;
            }
            self.decision_anchor_guidance.extend(guidance);
            denials
        } else {
            vec![None; calls.len()]
        };
        if active_handoff.is_some() {
            self.refresh_active_root_handoff(active_handoff);
        }
        for ((call, denial), recovery_reference_disposition) in calls
            .into_iter()
            .zip(denials)
            .zip(recovery_reference_dispositions)
        {
            let incomplete_staged_selector = self.incomplete_graph_selectors.contains_key(&call.id)
                && matches!(denial, Some(ToolCallDenial::GraphExplorationClosed(_)));
            let rejection = (!incomplete_staged_selector)
                .then(|| self.invocation_rejections.get(&call.id).cloned())
                .flatten();
            let shell_discovery_disposition = (rejection.is_none()
                && call.name == "bash"
                && matches!(&denial, Some(ToolCallDenial::DecisionAnchorMutation)))
            .then(ShellDiscoveryDispositionV1::excluded_never_executed_local_policy_denial);
            // A locally rejected or denied call must not expose either
            // shell-rendered argument presentation to activity.
            let presentation = if rejection.is_none() && denial.is_none() {
                self.tool_start_presentation
                    .as_ref()
                    .map(|render| render(&call.name, &call.arguments))
                    .unwrap_or_default()
            } else {
                ToolStartPresentation::default()
            };
            let operation_generation = self.next_operation_generation();
            operations.insert(call.id.clone(), operation_generation);
            requests.push(AgentRequest::Emit(AgentEvent::ToolStart {
                id: call.id.clone(),
                name: call.name.clone(),
                arg_preview: presentation.arg_preview,
                diagnostic_arguments: presentation.diagnostic_arguments,
                shell_discovery_disposition,
                recovery_reference_disposition,
            }));
            let redirect = (rejection.is_none() && denial.is_none())
                .then(|| self.ordinary_failures.redirect_for(&call))
                .flatten();
            if let Some(failure) = redirect {
                requests.push(AgentRequest::RedirectTool {
                    operation_generation,
                    batch_generation,
                    call,
                    failure,
                });
            } else {
                requests.push(AgentRequest::RunTool {
                    operation_generation,
                    batch_generation,
                    call,
                    denial,
                    rejection,
                });
            }
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
        failure: Option<ToolFailureDiagnostic>,
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

        // Resolve a typed source target only after the trusted wrapper has
        // returned its valid lineage. The opaque outcome remains attached to
        // this in-memory pending call and is never projected into protocol.
        let is_typed_source_call = self.pending_batches.front().is_some_and(|batch| {
            batch.iter().any(|pending| {
                pending.call.id == id
                    && pending.call.name == GraphCorrelationToolV1::GetCodeSnippet.public_name()
            })
        });
        let source_target = (is_typed_source_call
            && !output.is_error
            && failure.is_none()
            && !self.invocation_rejections.contains_key(&id))
        .then(|| {
            let details = output.details.as_ref()?;
            let correlation = serde_json::from_value::<GraphCorrelationV1>(
                details.get(SAFE_GRAPH_CORRELATION_DETAIL_KEY)?.clone(),
            )
            .ok()?;
            let lineage = serde_json::from_value::<DecisionAnchorLineageV1>(
                details
                    .get(SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY)?
                    .clone(),
            )
            .ok()?;
            (correlation.is_valid()
                && correlation.tool == GraphCorrelationToolV1::GetCodeSnippet
                && lineage.is_valid_for(&correlation))
            .then(|| {
                self.lineage_admission
                    .as_ref()
                    .map(|admission| admission.resolve_source_target(&lineage))
            })
            .flatten()
        })
        .flatten();

        // Record the result into the in-flight (front) batch.
        let mut completed_call = None;
        if let Some(batch) = self.pending_batches.front_mut() {
            if let Some(pending) = batch.iter_mut().find(|p| p.call.id == id) {
                if !self.invocation_rejections.contains_key(&id) {
                    completed_call = Some(pending.call.clone());
                }
                pending.source_target = source_target;
                pending.output = Some(output);
                pending.failure = failure.clone();
            }
        }
        if let Some(call) = completed_call {
            self.ordinary_failures
                .record_outcome(&call, failure.as_ref());
        }

        let batch_done = active.settled.len() == active.operations.len();
        if !batch_done {
            return requests;
        }
        self.active_tool_batch = None;

        // Retire the batch: its results join the turn's results in original
        // tool-call order (batches were planned in order, so appending preserves
        // it). Decision-anchor transitions are also evaluated here, rather
        // than on each transport completion, so a parallel graph batch always
        // sees complete results in its original dispatch order.
        if let Some(batch) = self.pending_batches.pop_front() {
            let active_handoff = if let Some(state) = self.decision_anchors.as_mut() {
                let completed = batch
                    .iter()
                    .filter_map(|pending| {
                        pending.output.as_ref().map(|output| {
                            (
                                pending.call.id.as_str(),
                                pending.call.name.as_str(),
                                output,
                                pending.source_target.as_ref(),
                                !output.is_error && pending.failure.is_none(),
                            )
                        })
                    })
                    .collect::<Vec<_>>();
                let transition = state.on_tool_batch_finished_with_targets(&completed);
                match transition {
                    DecisionAnchorTransition::Unchanged => {}
                    DecisionAnchorTransition::RecoveryNeeded => {
                        self.decision_anchor_recovery = true;
                    }
                    DecisionAnchorTransition::GapRecoveryNeeded => {
                        self.decision_anchor_gap_recovery = state.recovery_details();
                    }
                    DecisionAnchorTransition::EnabledEvidenceIncomplete => {
                        self.decision_anchor_incomplete = true;
                    }
                    DecisionAnchorTransition::ProviderUnavailableFallback => {
                        self.decision_anchor_guidance.push(
                            DECISION_ANCHOR_PROVIDER_UNAVAILABLE_FALLBACK_MESSAGE.to_string(),
                        );
                    }
                    DecisionAnchorTransition::EnabledEvidenceComplete => {
                        self.decision_anchor_complete =
                            !state.implementation_correction_decision_pending();
                    }
                }
                let active_handoff = state.active_recovery_action();
                self.decision_anchor_guidance
                    .extend(state.take_model_guidance());
                active_handoff
            } else {
                None
            };
            self.refresh_active_root_handoff(active_handoff);
            for pending in &batch {
                self.invocation_rejections.remove(&pending.call.id);
                self.incomplete_graph_selectors.remove(&pending.call.id);
                self.rejected_graph_tools.remove(&pending.call.id);
                self.recovery_reference_dispositions
                    .remove(&pending.call.id);
            }
            self.turn_results.extend(batch);
        }

        if self.decision_anchor_incomplete {
            requests.extend(self.finish(AgentStop::DecisionAnchorRecoveryExhausted));
            return requests;
        }

        if !self.pending_batches.is_empty() {
            requests.extend(self.dispatch_current_batch());
            return requests;
        }

        // All batches done: append every tool-result message in order, then
        // begin the next model turn.
        for pending in std::mem::take(&mut self.turn_results) {
            if let Some(output) = pending.output {
                self.messages.push(Message::ToolResult(std::sync::Arc::new(
                    tool_result_message(
                        &pending.call.id,
                        &pending.call.name,
                        output,
                        pending.failure,
                    ),
                )));
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
                failure,
            } => self.on_tool_finished(operation_generation, batch_generation, id, output, failure),
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
pub(super) fn extract_tool_calls(content: &[ContentBlock]) -> Vec<ToolCall> {
    content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolCall(call) => Some(call.clone()),
            _ => None,
        })
        .collect()
}
