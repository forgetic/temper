//! Privacy-safe, per-run enforcement for codebase-memory decision anchors.
//!
//! The policy never retains provider text, model arguments, paths, source, or
//! target digests. The trusted wrapper resolves provider-shaped selections in
//! process and hands this state only a bounded typed lineage record.

use std::collections::{BTreeMap, BTreeSet};

use temper_protocol_activity::{
    DecisionAnchorLineageStageV1, DecisionAnchorLineageV1, DecisionAnchorTargetKindV1,
    DecisionEvidenceKindV1, GraphCorrelationToolV1, GraphCorrelationV1,
};
use tongs::model::ToolCall;
use tongs::tools::{ToolEffects, ToolOutput};

use super::protocol::{CODEBASE_MEMORY_TOOL_PREFIX, ToolCallDenial};

mod anchors;
mod evidence;
mod output;

use output::{
    anchor_output, graph_tool_for_name, has_incompatible_targeted_result, successful_graph_batch,
    trusted_unavailable_provider_output,
};

/// Reserved wrapper detail carrying a process-local-root-bound lineage record.
/// It is deliberately excluded from durable activity metadata.
pub const SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY: &str = "temper_decision_anchor_lineage_v1";
/// Fixed, model-visible explanation for a locally denied mutation.
pub const DECISION_ANCHOR_MUTATION_BLOCKED_MESSAGE: &str = "workspace mutation blocked until the successful decision anchor is consumed through later result-derived codebase-memory evidence for the implementation, caller/model, and focused behavioral tests";
/// Fixed, privacy-safe instruction queued exactly once when graph evidence is complete.
pub const DECISION_ANCHOR_CONVERGENCE_MESSAGE: &str = "graph exploration complete: stop codebase-memory exploration and produce the smallest role-appropriate product supported by the verified current-root evidence.";
/// Fixed, privacy-safe result for graph calls denied after convergence or exhaustion.
pub const CODEBASE_MEMORY_EXPLORATION_CLOSED_MESSAGE: &str = "codebase-memory exploration is closed for this run; continue with conventional tools; do not retry codebase-memory immediately; continue with read, grep, find, shell, or other conventional discovery instead";

/// Generic, privacy-safe correction injected after a successful result cannot
/// be consumed as the active anchor's typed descendant.
pub const DECISION_ANCHOR_RECOVERY_MESSAGE: &str = "decision-anchor recovery required: the successful graph result did not form a compatible current-root descendant. Do not mutate; make a later targeted recovery selection or stop without a product.";
/// Recovery is deliberately bounded: repeated unrelated or unconsumable results
/// must not spin the native agent into a mutation-free but landable-looking run.
const MAX_DECISION_ANCHOR_RECOVERY_ATTEMPTS: u8 = 2;
/// One discovery turn may return several independent evidence roots. Bound the
/// retained opaque forest so provider output cannot grow policy state without limit.
/// Sixteen covers the largest legal parallel read batch while remaining fixed.
const MAX_DECISION_ANCHOR_ROOTS: usize = 16;
/// Later turns may add only a small fixed number of independent roots.
pub(super) const MAX_LATER_DECISION_ANCHOR_ROOTS: usize = 4;
/// Successful graph batches without typed progress eventually close exploration.
const MAX_NON_PROGRESSING_GRAPH_BATCHES: u8 = 2;
/// Budget exhaustion preserves exactly enough attempts to fill every possible
/// trace/evidence gap once, without reopening broad graph exploration.
const MAX_DECISION_GAP_RECOVERY_CALLS: u8 = 4;

pub(super) struct DecisionAnchorState {
    mutation_tools: BTreeSet<String>,
    phase: Option<AnchorPhase>,
    calls: BTreeMap<String, PendingCodebaseCall>,
    exploration: ExplorationStatus,
    later_roots: usize,
    non_progressing_batches: u8,
}

enum AnchorPhase {
    Root(AnchorForest),
    Trail(Trail),
    Recovery(Recovery),
    GapRecovery(GapRecovery),
    Exhausted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExplorationStatus {
    Open,
    GapRecovery,
    Complete,
    BudgetExhausted,
}

struct AnchorForest {
    roots: BTreeMap<String, Anchor>,
    valid: bool,
    latest_produced_turn: usize,
    trace_root_turn: Option<usize>,
}

struct Anchor {
    produced_turn: usize,
    result_target_kinds: BTreeSet<DecisionAnchorTargetKindV1>,
}

struct Trail {
    anchors: AnchorForest,
    evidence: SourceEvidence,
}

struct Recovery {
    anchors: AnchorForest,
    evidence: SourceEvidence,
    attempts: u8,
}

struct GapRecovery {
    anchors: AnchorForest,
    evidence: SourceEvidence,
    remaining: u8,
}

#[derive(Default)]
struct SourceEvidence {
    refinement_seen: bool,
    trace_turn: Option<usize>,
    decision_kinds: BTreeSet<DecisionEvidenceKindV1>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DecisionGap {
    Trace,
    Evidence(DecisionEvidenceKindV1),
}

#[derive(Clone, Copy)]
struct PendingCodebaseCall {
    turn: usize,
    recovery_gap: Option<DecisionGap>,
}

struct FinishedCodebaseCall<'a> {
    call: PendingCodebaseCall,
    name: &'a str,
    output: &'a ToolOutput,
}

struct AnchorOutput {
    lineage: DecisionAnchorLineageV1,
    tool: GraphCorrelationToolV1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RootMerge {
    Progress(usize),
    NoProgress,
    LimitExceeded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DecisionAnchorTransition {
    Unchanged,
    RecoveryNeeded,
    GapRecoveryNeeded,
    RecoveryExhausted,
    Converged,
    ExplorationExhausted,
}

impl DecisionAnchorState {
    pub(super) fn from_effects(effects: &BTreeMap<String, ToolEffects>) -> Option<Self> {
        let mutation_tools = effects
            .iter()
            .filter(|(_, effect)| effect.writes)
            .map(|(name, _)| name.clone())
            .collect::<BTreeSet<_>>();
        let has_codebase_memory = effects
            .keys()
            .any(|name| name.starts_with(CODEBASE_MEMORY_TOOL_PREFIX));
        has_codebase_memory.then_some(Self {
            mutation_tools,
            phase: None,
            calls: BTreeMap::new(),
            exploration: ExplorationStatus::Open,
            later_roots: 0,
            non_progressing_batches: 0,
        })
    }

    pub(super) fn on_tool_dispatched(
        &mut self,
        call: &ToolCall,
        turn: usize,
    ) -> Option<ToolCallDenial> {
        if call.name.starts_with(CODEBASE_MEMORY_TOOL_PREFIX) {
            let call_key = GraphCorrelationV1::target_digest(&call.id);
            if self.exploration != ExplorationStatus::Open {
                if self.exploration != ExplorationStatus::GapRecovery || call_key.is_none() {
                    return Some(ToolCallDenial::GraphExplorationClosed);
                }
                let Some(gap) = DecisionGap::from_call(call) else {
                    return Some(ToolCallDenial::GraphExplorationClosed);
                };
                let already_pending = self
                    .calls
                    .values()
                    .any(|pending| pending.recovery_gap == Some(gap));
                let Some(AnchorPhase::GapRecovery(recovery)) = self.phase.as_mut() else {
                    return Some(ToolCallDenial::GraphExplorationClosed);
                };
                if recovery.remaining == 0
                    || already_pending
                    || !recovery.evidence.needs(gap)
                    || !recovery.anchors.supports(gap)
                {
                    return Some(ToolCallDenial::GraphExplorationClosed);
                }
                recovery.remaining = recovery.remaining.saturating_sub(1);
            }
            if let Some(call_key) = call_key {
                self.calls.insert(
                    call_key,
                    PendingCodebaseCall {
                        turn,
                        recovery_gap: DecisionGap::from_call(call),
                    },
                );
            }
        }
        if self.blocks_mutation(&call.name) {
            return Some(ToolCallDenial::DecisionAnchorMutation);
        }
        None
    }

    #[cfg(test)]
    pub(super) fn on_tool_finished(
        &mut self,
        id: &str,
        name: &str,
        output: &ToolOutput,
    ) -> DecisionAnchorTransition {
        self.on_tool_batch_finished(&[(id, name, output)])
    }

    /// Evaluates one completed read-only batch from its pre-batch root state.
    /// The executor collects every sibling before this policy runs, so neither
    /// transport completion timing nor the model's sibling call order can make
    /// a valid later trace/source evidence set ineligible. Root producers still
    /// cannot be consumed by a sibling from their own model turn.
    pub(super) fn on_tool_batch_finished(
        &mut self,
        completed: &[(&str, &str, &ToolOutput)],
    ) -> DecisionAnchorTransition {
        let finished = completed
            .iter()
            .filter_map(|(id, name, output)| {
                let call_key = GraphCorrelationV1::target_digest(id)?;
                self.calls
                    .remove(&call_key)
                    .map(|call| FinishedCodebaseCall { call, name, output })
            })
            .collect::<Vec<_>>();
        if finished.is_empty() {
            return DecisionAnchorTransition::Unchanged;
        }

        let prior_phase = self.phase.take();
        if prior_phase.is_none() {
            // Initial independent roots are collected from the complete batch;
            // their same-turn siblings can never consume them.
            return match AnchorForest::from_finished(&finished, None) {
                Some(anchors) => self.install_roots(anchors, 0, false),
                None if successful_graph_batch(&finished) => self.record_non_progress(None),
                None => DecisionAnchorTransition::Unchanged,
            };
        }

        match prior_phase {
            None => unreachable!("the empty phase returned above"),
            Some(AnchorPhase::Root(root)) => {
                self.advance_batch_or_recover(root, SourceEvidence::default(), &finished, 1)
            }
            Some(AnchorPhase::Trail(trail)) if trail.evidence.is_complete() => {
                self.phase = Some(AnchorPhase::Trail(trail));
                if self.exploration == ExplorationStatus::Open {
                    self.exploration = ExplorationStatus::Complete;
                    return DecisionAnchorTransition::Converged;
                }
                DecisionAnchorTransition::Unchanged
            }
            Some(AnchorPhase::Trail(trail)) => {
                self.advance_batch_or_recover(trail.anchors, trail.evidence, &finished, 1)
            }
            Some(AnchorPhase::Recovery(recovery)) => {
                // Only a forest with no usable typed selections may be
                // replaced by fresh later roots. A cross-root result cannot
                // substitute for an otherwise consumable current forest.
                let replacement_roots = if recovery.anchors.is_consumable() {
                    None
                } else {
                    AnchorForest::from_finished(
                        &finished,
                        Some(recovery.anchors.latest_produced_turn),
                    )
                };
                if let Some(anchors) = replacement_roots {
                    self.install_roots(anchors, recovery.attempts.saturating_add(1), true)
                } else {
                    self.advance_batch_or_recover(
                        recovery.anchors,
                        recovery.evidence,
                        &finished,
                        recovery.attempts.saturating_add(1),
                    )
                }
            }
            Some(AnchorPhase::GapRecovery(recovery)) => {
                self.advance_gap_recovery(recovery, &finished)
            }
            Some(AnchorPhase::Exhausted) => {
                self.phase = Some(AnchorPhase::Exhausted);
                self.exploration = ExplorationStatus::BudgetExhausted;
                DecisionAnchorTransition::Unchanged
            }
        }
    }

    fn install_roots(
        &mut self,
        anchors: AnchorForest,
        attempts: u8,
        later: bool,
    ) -> DecisionAnchorTransition {
        if later {
            let next_count = anchors.roots.len();
            if next_count > MAX_LATER_DECISION_ANCHOR_ROOTS.saturating_sub(self.later_roots) {
                return self.enter_gap_recovery(anchors, SourceEvidence::default());
            }
            self.later_roots = self.later_roots.saturating_add(next_count);
        }
        if anchors.is_consumable() {
            self.non_progressing_batches = 0;
            if let Some(trace_turn) = anchors.trace_root_turn {
                let mut evidence = SourceEvidence::default();
                evidence.record_trace(trace_turn);
                self.phase = Some(AnchorPhase::Trail(Trail { anchors, evidence }));
            } else {
                self.phase = Some(AnchorPhase::Root(anchors));
            }
            DecisionAnchorTransition::Unchanged
        } else {
            self.enter_recovery(anchors, SourceEvidence::default(), attempts)
        }
    }

    fn advance_batch_or_recover(
        &mut self,
        mut active: AnchorForest,
        evidence: SourceEvidence,
        finished: &[FinishedCodebaseCall<'_>],
        recovery_attempts: u8,
    ) -> DecisionAnchorTransition {
        // Every selection must be a later, root-bound typed descendant. Keep
        // the root anchor active throughout the evidence set: a valid trace or
        // source read need not manufacture a new provider result for its
        // siblings to remain eligible.
        let compatible = finished
            .iter()
            .filter_map(|finished| {
                let output = anchor_output(finished.name, finished.output)?;
                active
                    .accepts(&finished.call, &output.lineage)
                    .then_some((finished.call, output))
            })
            .collect::<Vec<_>>();
        // Capture roots produced by this batch only after descendants have
        // been evaluated against the pre-batch forest. They can be consumed by
        // later model turns, never by their own siblings. This also preserves
        // independent implementation/caller/test chains discovered over more
        // than one bounded batch.
        let next_roots = AnchorForest::from_finished(finished, Some(active.latest_produced_turn));
        let candidate_root_trace_turn = next_roots.as_ref().and_then(|roots| roots.trace_root_turn);
        let root_merge = next_roots.map_or(RootMerge::NoProgress, |next| {
            active.merge_limited(
                next,
                MAX_LATER_DECISION_ANCHOR_ROOTS.saturating_sub(self.later_roots),
            )
        });
        let batch_root_trace_turn = (root_merge != RootMerge::LimitExceeded)
            .then_some(candidate_root_trace_turn)
            .flatten();
        if let RootMerge::Progress(added) = root_merge {
            self.later_roots = self.later_roots.saturating_add(added);
        }
        let roots_progressed = matches!(root_merge, RootMerge::Progress(_));
        if !active.valid {
            return self.enter_recovery(active, evidence, recovery_attempts);
        }

        let batch_trace_turn = compatible
            .iter()
            .filter(|(_, output)| output.tool == GraphCorrelationToolV1::TracePath)
            .map(|(call, _)| call.turn)
            .chain(batch_root_trace_turn)
            .min();
        let trace_turn = evidence.trace_turn.or(batch_trace_turn);
        let decision_kinds = trace_turn.map_or_else(BTreeSet::new, |trace_turn| {
            compatible
                .iter()
                .filter_map(|(call, output)| {
                    (output.tool == GraphCorrelationToolV1::GetCodeSnippet
                        && call.turn >= trace_turn)
                        .then_some(output.lineage.decision_evidence_kind)
                        .flatten()
                })
                .collect()
        });
        let refinement = !evidence.refinement_seen
            && compatible
                .iter()
                .any(|(_, output)| output.tool == GraphCorrelationToolV1::SearchCode);
        let trace_progressed = !evidence.has_trace() && batch_trace_turn.is_some();
        let evidence_progressed = decision_kinds
            .iter()
            .any(|kind| !evidence.decision_kinds.contains(kind));
        let progressed = trace_progressed || evidence_progressed || refinement || roots_progressed;

        if progressed {
            self.non_progressing_batches = 0;
            let mut evidence = evidence;
            evidence.refinement_seen |= refinement;
            if let Some(trace_turn) = batch_trace_turn {
                evidence.record_trace(trace_turn);
            }
            evidence.record_decision_kinds(decision_kinds);
            let complete = evidence.is_complete();
            self.phase = Some(AnchorPhase::Trail(Trail {
                anchors: active,
                evidence,
            }));
            if complete {
                self.exploration = ExplorationStatus::Complete;
                return DecisionAnchorTransition::Converged;
            }
            if root_merge == RootMerge::LimitExceeded {
                let Some(AnchorPhase::Trail(trail)) = self.phase.take() else {
                    unreachable!("the incomplete trail was installed above")
                };
                return self.enter_gap_recovery(trail.anchors, trail.evidence);
            }
            return DecisionAnchorTransition::Unchanged;
        }

        if root_merge == RootMerge::LimitExceeded {
            return self.enter_gap_recovery(active, evidence);
        }

        if finished.iter().any(|finished| {
            trusted_unavailable_provider_output(finished.name, finished.output)
                && !active.contains_producer_turn(&finished.call)
                && evidence.expects(
                    finished.call.recovery_gap,
                    graph_tool_for_name(finished.name),
                    batch_trace_turn,
                )
        }) {
            // The trusted wrapper has already supplied fixed, bounded fallback
            // guidance. Release only an unavailable viable evidence step; an
            // unrelated failed graph discovery call cannot bypass this root.
            self.phase = None;
            self.exploration = ExplorationStatus::BudgetExhausted;
            return DecisionAnchorTransition::Unchanged;
        }

        if finished
            .iter()
            .any(|finished| active.contains_producer_turn(&finished.call))
        {
            self.phase = Some(AnchorPhase::Trail(Trail {
                anchors: active,
                evidence,
            }));
            return DecisionAnchorTransition::Unchanged;
        }

        // Valid repeated roots and broad successful discovery consume the
        // fixed non-progress budget. Typed but incompatible descendants (and
        // targeted results whose lineage is malformed or ambiguous) retain the
        // established bounded recovery behavior.
        if successful_graph_batch(finished) && !has_incompatible_targeted_result(finished, &active)
        {
            return self.record_non_progress(Some(AnchorPhase::Trail(Trail {
                anchors: active,
                evidence,
            })));
        }

        self.enter_recovery(active, evidence, recovery_attempts)
    }

    fn record_non_progress(&mut self, phase: Option<AnchorPhase>) -> DecisionAnchorTransition {
        self.non_progressing_batches = self.non_progressing_batches.saturating_add(1);
        if self.non_progressing_batches >= MAX_NON_PROGRESSING_GRAPH_BATCHES {
            return match phase {
                Some(AnchorPhase::Trail(trail)) => {
                    self.enter_gap_recovery(trail.anchors, trail.evidence)
                }
                Some(AnchorPhase::Root(anchors)) => {
                    self.enter_gap_recovery(anchors, SourceEvidence::default())
                }
                phase => {
                    self.phase = phase;
                    self.exploration = ExplorationStatus::BudgetExhausted;
                    DecisionAnchorTransition::ExplorationExhausted
                }
            };
        }
        self.phase = phase;
        DecisionAnchorTransition::Unchanged
    }

    fn enter_recovery(
        &mut self,
        anchors: AnchorForest,
        evidence: SourceEvidence,
        attempts: u8,
    ) -> DecisionAnchorTransition {
        if attempts >= MAX_DECISION_ANCHOR_RECOVERY_ATTEMPTS {
            self.phase = Some(AnchorPhase::Exhausted);
            self.exploration = ExplorationStatus::BudgetExhausted;
            DecisionAnchorTransition::RecoveryExhausted
        } else {
            self.phase = Some(AnchorPhase::Recovery(Recovery {
                anchors,
                evidence,
                attempts,
            }));
            DecisionAnchorTransition::RecoveryNeeded
        }
    }

    pub(super) fn blocks_mutation(&self, name: &str) -> bool {
        self.mutation_tools.contains(name)
            && self.phase.as_ref().is_some_and(|phase| match phase {
                AnchorPhase::Root(_) => true,
                AnchorPhase::Trail(trail) => !trail.evidence.is_complete(),
                AnchorPhase::Recovery(_) | AnchorPhase::GapRecovery(_) | AnchorPhase::Exhausted => {
                    true
                }
            })
    }
}
