//! Privacy-safe, per-run enforcement for codebase-memory decision anchors.
//!
//! The policy never retains provider text, model arguments, paths, source, or
//! target digests. The trusted wrapper resolves provider-shaped selections in
//! process and hands this state only a bounded typed lineage record.

use std::collections::{BTreeMap, BTreeSet};

use temper_protocol_activity::{
    DecisionAnchorLineageStageV1, DecisionAnchorLineageV1, DecisionAnchorTargetKindV1,
    DecisionEvidenceKindV1, GraphCorrelationToolV1, GraphCorrelationV1, GraphExplorationClosedV1,
    GraphRecoveryActionV1, GraphRecoveryEvidenceKindV1, MAX_GRAPH_RECOVERY_ALLOWANCE_V1,
};
use tongs::model::ToolCall;
use tongs::tools::{ToolEffects, ToolOutput};

use super::protocol::{CODEBASE_MEMORY_TOOL_PREFIX, ToolCallDenial};

mod admission;
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
const MAX_DECISION_ANCHOR_ROOTS: usize = 16;
/// Later turns may add only a small fixed number of independent roots.
pub(super) const MAX_LATER_DECISION_ANCHOR_ROOTS: usize = 4;
/// Successful graph batches without typed progress eventually close exploration.
const MAX_NON_PROGRESSING_GRAPH_BATCHES: u8 = 2;
/// Budget exhaustion preserves exactly enough attempts to fill every possible
/// trace/evidence gap once, without reopening broad graph exploration.
const MAX_DECISION_GAP_RECOVERY_CALLS: u8 = MAX_GRAPH_RECOVERY_ALLOWANCE_V1;

pub(super) struct DecisionAnchorState {
    mutation_tools: BTreeSet<String>,
    phase: Option<AnchorPhase>,
    calls: BTreeMap<String, PendingCodebaseCall>,
    exploration: ExplorationStatus,
    next_call_order: u64,
    later_roots: usize,
    non_progressing_batches: u8,
}

enum AnchorPhase {
    Root(AnchorForest),
    Trail(AnchorForest),
    Recovery(Recovery),
    GapRecovery(GapRecovery),
    Exhausted(SourceEvidence),
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
}

struct Anchor {
    produced_turn: usize,
    produced_order: u64,
    result_target_kinds: BTreeSet<DecisionAnchorTargetKindV1>,
    evidence: SourceEvidence,
}

struct Recovery {
    anchors: AnchorForest,
    attempts: u8,
}

struct GapRecovery {
    anchors: AnchorForest,
    active_root: String,
    remaining: u8,
}

#[derive(Clone, Default)]
struct SourceEvidence {
    trace_turn: Option<usize>,
    decision_kinds: BTreeSet<DecisionEvidenceKindV1>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum DecisionGap {
    Trace,
    Evidence(DecisionEvidenceKindV1),
}

#[derive(Clone)]
struct PendingCodebaseCall {
    turn: usize,
    order: u64,
    recovery_gap: Option<DecisionGap>,
    admitted_root: Option<String>,
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
            next_call_order: 0,
            later_roots: 0,
            non_progressing_batches: 0,
        })
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
    /// The executor collects every sibling before this policy runs, so result
    /// settlement is independent of transport completion timing.
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
            return match AnchorForest::from_finished(&finished, None) {
                Some(anchors) => self.install_roots(anchors, 0, false),
                None if successful_graph_batch(&finished) => self.record_non_progress(None),
                None => DecisionAnchorTransition::Unchanged,
            };
        }

        match prior_phase {
            None => unreachable!("the empty phase returned above"),
            Some(AnchorPhase::Root(anchors)) | Some(AnchorPhase::Trail(anchors)) => {
                if anchors.has_complete_evidence() {
                    self.phase = Some(AnchorPhase::Trail(anchors));
                    if self.exploration == ExplorationStatus::Open {
                        self.exploration = ExplorationStatus::Complete;
                        DecisionAnchorTransition::Converged
                    } else {
                        DecisionAnchorTransition::Unchanged
                    }
                } else {
                    self.advance_batch_or_recover(anchors, &finished, 1)
                }
            }
            Some(AnchorPhase::Recovery(recovery)) => {
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
                        &finished,
                        recovery.attempts.saturating_add(1),
                    )
                }
            }
            Some(AnchorPhase::GapRecovery(recovery)) => {
                self.advance_gap_recovery(recovery, &finished)
            }
            Some(AnchorPhase::Exhausted(evidence)) => {
                self.phase = Some(AnchorPhase::Exhausted(evidence));
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
                return self.enter_gap_recovery(anchors);
            }
            self.later_roots = self.later_roots.saturating_add(next_count);
        }
        if anchors.is_consumable() {
            self.non_progressing_batches = 0;
            self.phase = Some(if anchors.has_any_evidence() {
                AnchorPhase::Trail(anchors)
            } else {
                AnchorPhase::Root(anchors)
            });
            DecisionAnchorTransition::Unchanged
        } else {
            self.enter_recovery(anchors, attempts)
        }
    }

    fn advance_batch_or_recover(
        &mut self,
        mut anchors: AnchorForest,
        finished: &[FinishedCodebaseCall<'_>],
        recovery_attempts: u8,
    ) -> DecisionAnchorTransition {
        let compatible = finished
            .iter()
            .filter_map(|finished| {
                let output = anchor_output(finished.name, finished.output)?;
                anchors
                    .accepted_root(&finished.call, &output.lineage)
                    .map(|root| (root, finished.call.clone(), output))
            })
            .collect::<Vec<_>>();
        let batch_trace_turns = compatible
            .iter()
            .filter(|(_, _, output)| output.tool == GraphCorrelationToolV1::TracePath)
            .fold(BTreeMap::new(), |mut turns, (root, call, _)| {
                turns
                    .entry(root.clone())
                    .and_modify(|turn: &mut usize| *turn = (*turn).min(call.turn))
                    .or_insert(call.turn);
                turns
            });
        let mut evidence_progressed = false;
        for (root, call, output) in &compatible {
            let Some(anchor) = anchors.roots.get_mut(root) else {
                continue;
            };
            let before = anchor.evidence.progress_count();
            match output.tool {
                GraphCorrelationToolV1::TracePath => anchor.evidence.record_trace(call.turn),
                GraphCorrelationToolV1::SearchCode => anchor
                    .evidence
                    .record_decision_kinds([DecisionEvidenceKindV1::Implementation]),
                GraphCorrelationToolV1::GetCodeSnippet
                    if anchor.evidence.has_trace()
                        || batch_trace_turns
                            .get(root)
                            .is_some_and(|trace_turn| call.turn >= *trace_turn) =>
                {
                    anchor
                        .evidence
                        .record_decision_kinds(output.lineage.decision_evidence_kind)
                }
                GraphCorrelationToolV1::SearchGraph | GraphCorrelationToolV1::GetCodeSnippet => {}
            }
            evidence_progressed |= anchor.evidence.progress_count() > before;
        }

        // New roots are retained only after descendants were checked against
        // the pre-batch forest, so siblings can never consume a new root.
        let next_roots = AnchorForest::from_finished(finished, Some(anchors.latest_produced_turn));
        let root_merge = next_roots.map_or(RootMerge::NoProgress, |next| {
            anchors.merge_limited(
                next,
                MAX_LATER_DECISION_ANCHOR_ROOTS.saturating_sub(self.later_roots),
            )
        });
        if let RootMerge::Progress(added) = root_merge {
            self.later_roots = self.later_roots.saturating_add(added);
        }
        let roots_progressed = matches!(root_merge, RootMerge::Progress(_));
        if !anchors.valid {
            return self.enter_recovery(anchors, recovery_attempts);
        }

        if evidence_progressed || roots_progressed {
            self.non_progressing_batches = 0;
            let complete = anchors.has_complete_evidence();
            self.phase = Some(AnchorPhase::Trail(anchors));
            if complete {
                self.exploration = ExplorationStatus::Complete;
                return DecisionAnchorTransition::Converged;
            }
            if root_merge == RootMerge::LimitExceeded {
                let Some(AnchorPhase::Trail(anchors)) = self.phase.take() else {
                    unreachable!("the incomplete trail was installed above")
                };
                return self.enter_gap_recovery(anchors);
            }
            return DecisionAnchorTransition::Unchanged;
        }

        if root_merge == RootMerge::LimitExceeded {
            return self.enter_gap_recovery(anchors);
        }

        if finished.iter().any(|finished| {
            trusted_unavailable_provider_output(finished.name, finished.output)
                && !anchors.contains_producer_turn(&finished.call)
                && anchors.expected_for_call(
                    &finished.call,
                    finished.call.recovery_gap,
                    graph_tool_for_name(finished.name),
                )
        }) {
            self.phase = None;
            self.exploration = ExplorationStatus::BudgetExhausted;
            return DecisionAnchorTransition::Unchanged;
        }

        if finished
            .iter()
            .any(|finished| anchors.contains_producer_turn(&finished.call))
        {
            self.phase = Some(AnchorPhase::Trail(anchors));
            return DecisionAnchorTransition::Unchanged;
        }

        if successful_graph_batch(finished) && !has_incompatible_targeted_result(finished, &anchors)
        {
            return self.record_non_progress(Some(AnchorPhase::Trail(anchors)));
        }

        self.enter_recovery(anchors, recovery_attempts)
    }

    fn record_non_progress(&mut self, phase: Option<AnchorPhase>) -> DecisionAnchorTransition {
        self.non_progressing_batches = self.non_progressing_batches.saturating_add(1);
        if self.non_progressing_batches >= MAX_NON_PROGRESSING_GRAPH_BATCHES {
            return match phase {
                Some(AnchorPhase::Trail(anchors)) | Some(AnchorPhase::Root(anchors)) => {
                    self.enter_gap_recovery(anchors)
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

    fn enter_recovery(&mut self, anchors: AnchorForest, attempts: u8) -> DecisionAnchorTransition {
        if attempts >= MAX_DECISION_ANCHOR_RECOVERY_ATTEMPTS {
            self.phase = Some(AnchorPhase::Exhausted(anchors.active_evidence()));
            self.exploration = ExplorationStatus::BudgetExhausted;
            DecisionAnchorTransition::RecoveryExhausted
        } else {
            self.phase = Some(AnchorPhase::Recovery(Recovery { anchors, attempts }));
            DecisionAnchorTransition::RecoveryNeeded
        }
    }

    pub(super) fn recovery_details(&self) -> Option<GraphExplorationClosedV1> {
        let AnchorPhase::GapRecovery(recovery) = self.phase.as_ref()? else {
            return None;
        };
        let active = recovery.anchors.roots.get(&recovery.active_root)?;
        let compatible_actions = active
            .evidence
            .compatible_gaps(active)
            .into_iter()
            .map(|gap| GraphRecoveryActionV1::for_evidence(gap.recovery_kind()))
            .collect::<Vec<_>>();
        GraphExplorationClosedV1::recoverable_with_actions(
            active.evidence.missing_kinds(),
            recovery.remaining,
            compatible_actions,
        )
    }

    fn recovery_denial_details(&self) -> Option<GraphExplorationClosedV1> {
        let AnchorPhase::GapRecovery(recovery) = self.phase.as_ref()? else {
            return None;
        };
        let active = recovery.anchors.roots.get(&recovery.active_root)?;
        GraphExplorationClosedV1::recoverable_without_actions(
            active.evidence.missing_kinds(),
            recovery.remaining,
        )
    }

    fn graph_exploration_denial(&self) -> ToolCallDenial {
        let details = match self.exploration {
            ExplorationStatus::Complete => Some(GraphExplorationClosedV1::completed()),
            ExplorationStatus::GapRecovery => self.recovery_denial_details().or_else(|| {
                let AnchorPhase::GapRecovery(recovery) = self.phase.as_ref()? else {
                    return None;
                };
                let active = recovery.anchors.roots.get(&recovery.active_root)?;
                GraphExplorationClosedV1::exhausted(active.evidence.missing_kinds())
            }),
            ExplorationStatus::BudgetExhausted => match self.phase.as_ref() {
                Some(AnchorPhase::Exhausted(evidence)) => {
                    GraphExplorationClosedV1::exhausted(evidence.missing_kinds())
                }
                _ => None,
            },
            ExplorationStatus::Open => None,
        };
        ToolCallDenial::GraphExplorationClosed(details)
    }

    pub(super) fn blocks_mutation(&self, name: &str) -> bool {
        self.mutation_tools.contains(name)
            && self.phase.as_ref().is_some_and(|phase| match phase {
                AnchorPhase::Root(_) => true,
                AnchorPhase::Trail(anchors) => !anchors.has_complete_evidence(),
                AnchorPhase::Recovery(_)
                | AnchorPhase::GapRecovery(_)
                | AnchorPhase::Exhausted(_) => true,
            })
    }
}
