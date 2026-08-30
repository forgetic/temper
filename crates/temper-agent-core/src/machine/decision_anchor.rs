//! Privacy-safe, per-run enforcement for codebase-memory decision anchors.
//!
//! The policy never retains provider text, model arguments, paths, source, or
//! target digests. The trusted wrapper resolves provider-shaped selections in
//! process and hands this state only a bounded typed lineage record.

use std::collections::{BTreeMap, BTreeSet};

use temper_protocol_activity::{
    CallerDiscoveryOutcomeV1, DecisionAnchorLineageStageV1, DecisionAnchorLineageV1,
    DecisionAnchorTargetKindV1, DecisionEvidenceKindV1, FocusedTestDiscoveryOutcomeV1,
    GraphCorrelationToolV1, GraphCorrelationV1, GraphExplorationClosedV1, GraphRecoveryActionV1,
    GraphRecoveryEvidenceKindV1, MAX_GRAPH_RECOVERY_ALLOWANCE_V1,
};
use tongs::model::ToolCall;
use tongs::tools::{ToolEffects, ToolOutput};

use crate::{EligibleWorkspaceTarget, InvocationTargetAdmission, TargetAdmissionOutcome};

use super::protocol::{CODEBASE_MEMORY_TOOL_PREFIX, ToolCallDenial};

mod actions;
mod admission;
mod anchors;
mod evidence;
mod exact_read;
mod output;
mod progress;
mod settlement;

use output::{
    anchor_output, graph_tool_for_name, has_incompatible_targeted_result, successful_graph_batch,
    trusted_unavailable_provider_output,
};
use progress::{AcceptedEvidence, ResultProgress};

/// Reserved wrapper detail carrying a process-local-root-bound lineage record.
/// It is deliberately excluded from durable activity metadata.
pub const SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY: &str = "temper_decision_anchor_lineage_v1";
/// Fixed, model-visible explanation for a locally denied mutation.
pub const DECISION_ANCHOR_MUTATION_BLOCKED_MESSAGE: &str = "workspace mutation blocked: use the ordinary read tool to read the exact target named by this mutation after either its qualifying graph source result has completed or conventional fallback has been released, then retry the mutation";
/// Fixed, privacy-safe instruction queued exactly once when graph evidence is complete.
pub const DECISION_ANCHOR_CONVERGENCE_MESSAGE: &str = "graph exploration complete: stop codebase-memory exploration, use the ordinary read tool to read the exact workspace target selected by the qualifying graph source result, and only then mutate that matching target.";
/// Fixed, privacy-safe result for graph calls denied after convergence or exhaustion.
pub const CODEBASE_MEMORY_EXPLORATION_CLOSED_MESSAGE: &str = "codebase-memory exploration is closed for this run; continue with conventional tools; do not retry codebase-memory immediately; continue with read, grep, find, shell, or other conventional discovery instead";
/// Fixed bounded fallback released only after trusted systemic provider unavailability.
pub const DECISION_ANCHOR_PROVIDER_UNAVAILABLE_FALLBACK_MESSAGE: &str = "codebase-memory is systemically unavailable and graph exploration is closed for this run. Do not retry codebase-memory. Perform only the existing minimal conventional fallback: one simple discovery command plus a necessary directory change, if any; then use the ordinary read tool to read the exact source target before making only the matching minimal mutation. Continue through the normal validation and submission gates.";

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
/// Bounds opaque source, pending-read, and successful-read authority.
const MAX_EXACT_TARGET_AUTHORITIES: usize = 64;

pub(super) struct DecisionAnchorState {
    mutation_tools: BTreeSet<String>,
    phase: Option<AnchorPhase>,
    calls: BTreeMap<String, PendingCodebaseCall>,
    exploration: ExplorationStatus,
    next_call_order: u64,
    later_roots: usize,
    non_progressing_batches: u8,
    source_authorities: Vec<SourceTargetAuthority>,
    pending_exact_reads: BTreeMap<String, PendingExactRead>,
    exact_read_authorities: Vec<ExactReadAuthority>,
    settled_batches: u64,
    batch_progress: BTreeMap<String, ResultProgress>,
    model_guidance: Vec<String>,
    rejected_recovery_tuples: BTreeSet<RecoveryTupleIdentity>,
    pending_conventional_reads: BTreeMap<String, EligibleWorkspaceTarget>,
    conventional_read_authorities: Vec<EligibleWorkspaceTarget>,
}

enum AnchorPhase {
    Root(AnchorForest),
    Trail(AnchorForest),
    Recovery(Recovery),
    GapRecovery(GapRecovery),
    /// Complete enabled evidence awaiting its exact ordinary source read.
    EnabledComplete(AnchorForest),
    /// Enabled graph activity which exhausted bounded recovery while incomplete.
    EnabledIncomplete(SourceEvidence),
    /// Trusted systemic graph unavailability with the predecessor fallback policy.
    ProviderUnavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExplorationStatus {
    Open,
    GapRecovery,
    EnabledComplete,
    EnabledIncomplete,
    ProviderUnavailable,
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
    exact_graph_narrowing_selected: bool,
    evidence: SourceEvidence,
}

struct Recovery {
    anchors: AnchorForest,
    attempts: u8,
}

struct GapRecovery {
    anchors: AnchorForest,
    active_root: String,
    route: RecoveryRoute,
    remaining: u8,
    exhausted_roots: BTreeSet<String>,
    remaining_pivots: usize,
}

#[derive(Clone)]
struct SourceTargetAuthority {
    target: EligibleWorkspaceTarget,
    root_binding: String,
    completed_turn: usize,
    completed_order: u64,
    completed_batch: u64,
}

struct PendingExactRead {
    sources: Vec<SourceTargetAuthority>,
    dispatched_turn: usize,
    dispatched_order: u64,
    dispatched_after_batch: u64,
}

struct ExactReadAuthority {
    target: EligibleWorkspaceTarget,
    root_binding: String,
    source_completed_turn: usize,
    source_completed_order: u64,
    source_completed_batch: u64,
    read_dispatched_turn: usize,
    read_dispatched_order: u64,
    read_dispatched_after_batch: u64,
}

#[derive(Clone, Default)]
struct SourceEvidence {
    trace_turn: Option<usize>,
    decision_kinds: BTreeSet<DecisionEvidenceKindV1>,
    caller_selector_available: bool,
    trace_before_implementation: bool,
    caller_traversal_outcome: Option<CallerDiscoveryOutcomeV1>,
    focused_test_selector_available: bool,
    focused_test_traversal_turn: Option<usize>,
    focused_test_traversal_outcome: Option<FocusedTestDiscoveryOutcomeV1>,
    focused_test_fallback_turn: Option<usize>,
    focused_test_fallback_outcome: Option<FocusedTestDiscoveryOutcomeV1>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RecoveryRoute {
    Implementation,
    FocusedTest,
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
    admission_checked: bool,
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub(in crate::machine) struct RecoveryTupleIdentity([u8; 32]);

type SettledToolCall<'a> = (
    &'a str,
    &'a str,
    &'a ToolOutput,
    Option<&'a TargetAdmissionOutcome>,
    bool,
);

struct FinishedCodebaseCall<'a> {
    id: &'a str,
    call: PendingCodebaseCall,
    name: &'a str,
    output: &'a ToolOutput,
    source_target: Option<&'a TargetAdmissionOutcome>,
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
    EnabledEvidenceComplete,
    EnabledEvidenceIncomplete,
    ProviderUnavailableFallback,
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
            source_authorities: Vec::new(),
            pending_exact_reads: BTreeMap::new(),
            exact_read_authorities: Vec::new(),
            settled_batches: 0,
            batch_progress: BTreeMap::new(),
            model_guidance: Vec::new(),
            rejected_recovery_tuples: BTreeSet::new(),
            pending_conventional_reads: BTreeMap::new(),
            conventional_read_authorities: Vec::new(),
        })
    }

    #[cfg(test)]
    pub(super) fn on_tool_finished(
        &mut self,
        id: &str,
        name: &str,
        output: &ToolOutput,
    ) -> DecisionAnchorTransition {
        self.on_tool_batch_finished_with_targets(&[(id, name, output, None, !output.is_error)])
    }

    #[cfg(test)]
    pub(super) fn on_tool_finished_with_source_target(
        &mut self,
        id: &str,
        name: &str,
        output: &ToolOutput,
        source_target: Option<&TargetAdmissionOutcome>,
    ) -> DecisionAnchorTransition {
        self.on_tool_batch_finished_with_targets(&[(
            id,
            name,
            output,
            source_target,
            !output.is_error,
        )])
    }

    /// Evaluates one completed read-only batch from its pre-batch root state.
    /// The executor collects every sibling before this policy runs, so result
    /// settlement is independent of transport completion timing.
    #[cfg(test)]
    pub(super) fn on_tool_batch_finished(
        &mut self,
        completed: &[(&str, &str, &ToolOutput)],
    ) -> DecisionAnchorTransition {
        let completed = completed
            .iter()
            .map(|(id, name, output)| (*id, *name, *output, None, !output.is_error))
            .collect::<Vec<_>>();
        self.on_tool_batch_finished_with_targets(&completed)
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
        let selected_active_root = anchors
            .active_selection(&BTreeSet::new())
            .map(|(binding, _)| binding.clone());
        let compatible = finished
            .iter()
            .filter_map(|finished| {
                if finished.call.admission_checked && finished.call.admitted_root.is_none() {
                    return None;
                }
                let output = anchor_output(finished.name, finished.output)?;
                anchors
                    .accepted_root(&finished.call, &output.lineage)
                    .map(|root| {
                        (
                            root,
                            finished.id,
                            finished.call.clone(),
                            output,
                            finished.source_target,
                        )
                    })
            })
            .collect::<Vec<_>>();
        let mut evidence_progressed = false;
        let mut active_evidence_progressed = false;
        let mut route_progressed = false;
        let mut active_route_progressed = false;
        for (root, id, call, output, source_target) in &compatible {
            let Some(anchor) = anchors.roots.get_mut(root) else {
                continue;
            };
            let before = anchor.evidence.progress_count();
            let target_kinds_before = anchor.result_target_kinds.len();
            let had_trace = anchor.evidence.has_trace();
            let had_caller_selector = anchor.evidence.caller_selector_available;
            let had_focused_test_selector = anchor.evidence.focused_test_selector_available;
            let had_implementation = anchor
                .evidence
                .decision_kinds
                .contains(&DecisionEvidenceKindV1::Implementation);
            anchor
                .result_target_kinds
                .extend(output.lineage.result_target_kinds.iter().copied());
            if anchor.result_target_kinds.len() > target_kinds_before {
                route_progressed = true;
                active_route_progressed |= selected_active_root.as_ref() == Some(root);
                self.mark_route_progress(id);
            }
            let mut accepted_source = false;
            match output.tool {
                GraphCorrelationToolV1::TracePath
                    if call.recovery_gap
                        == Some(DecisionGap::Evidence(DecisionEvidenceKindV1::FocusedTest)) =>
                {
                    anchor.evidence.record_focused_test_traversal(call.turn);
                    anchor.evidence.record_focused_test_discovery(
                        output.tool,
                        output.lineage.focused_test_discovery,
                    );
                    self.mark_accepted(id, AcceptedEvidence::FocusedTestRoute);
                }
                GraphCorrelationToolV1::TracePath if output.lineage.caller_discovery.is_some() => {
                    anchor.evidence.record_trace(call.turn);
                    anchor
                        .evidence
                        .record_caller_discovery(output.lineage.caller_discovery);
                    self.mark_accepted(id, AcceptedEvidence::Trace);
                }
                GraphCorrelationToolV1::GetCodeSnippet => {
                    match output.lineage.decision_evidence_kind {
                        Some(DecisionEvidenceKindV1::Implementation) => {
                            anchor
                                .evidence
                                .record_decision_kinds([DecisionEvidenceKindV1::Implementation]);
                            accepted_source = true;
                            self.mark_accepted(id, AcceptedEvidence::Implementation);
                        }
                        Some(DecisionEvidenceKindV1::Caller)
                            if had_trace
                                && (had_caller_selector || !call.admission_checked)
                                && (had_implementation || !call.admission_checked) =>
                        {
                            anchor
                                .evidence
                                .record_decision_kinds([DecisionEvidenceKindV1::Caller]);
                            accepted_source = true;
                            self.mark_accepted(id, AcceptedEvidence::Caller);
                        }
                        Some(DecisionEvidenceKindV1::FocusedTest)
                            if had_focused_test_selector || !call.admission_checked =>
                        {
                            anchor
                                .evidence
                                .record_decision_kinds([DecisionEvidenceKindV1::FocusedTest]);
                            accepted_source = true;
                            self.mark_accepted(id, AcceptedEvidence::FocusedTest);
                        }
                        Some(_) | None => {}
                    }
                }
                GraphCorrelationToolV1::SearchGraph
                    if call.recovery_gap
                        == Some(DecisionGap::Evidence(DecisionEvidenceKindV1::FocusedTest)) =>
                {
                    anchor.evidence.record_focused_test_fallback(call.turn);
                    anchor.evidence.record_focused_test_discovery(
                        output.tool,
                        output.lineage.focused_test_discovery,
                    );
                    self.mark_accepted(id, AcceptedEvidence::FocusedTestRoute);
                }
                GraphCorrelationToolV1::SearchGraph
                    if matches!(
                        output.lineage.target_kind,
                        DecisionAnchorTargetKindV1::NamePattern
                            | DecisionAnchorTargetKindV1::QualifiedNamePattern
                    ) =>
                {
                    if !anchor.exact_graph_narrowing_selected {
                        anchor.exact_graph_narrowing_selected = true;
                        route_progressed = true;
                        active_route_progressed |= selected_active_root.as_ref() == Some(root);
                        self.mark_route_progress(id);
                    }
                }
                GraphCorrelationToolV1::SearchGraph | GraphCorrelationToolV1::SearchCode => {}
                GraphCorrelationToolV1::TracePath => {}
            }
            if accepted_source {
                self.record_source_authority(root, call, *source_target);
            }
            let root_progressed = anchor.evidence.progress_count() > before;
            evidence_progressed |= root_progressed;
            active_evidence_progressed |=
                root_progressed && selected_active_root.as_ref() == Some(root);
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

        if active_evidence_progressed || active_route_progressed || roots_progressed {
            self.non_progressing_batches = 0;
            if anchors.has_complete_evidence() {
                self.phase = Some(AnchorPhase::EnabledComplete(anchors));
                self.exploration = ExplorationStatus::EnabledComplete;
                return DecisionAnchorTransition::EnabledEvidenceComplete;
            }
            if !anchors.has_compatible_actions() {
                return self.enter_gap_recovery(anchors);
            }
            self.phase = Some(AnchorPhase::Trail(anchors));
            if root_merge == RootMerge::LimitExceeded {
                let Some(AnchorPhase::Trail(anchors)) = self.phase.take() else {
                    unreachable!("the incomplete trail was installed above")
                };
                return self.enter_gap_recovery(anchors);
            }
            return DecisionAnchorTransition::Unchanged;
        }

        if evidence_progressed || route_progressed {
            return self.record_non_progress(Some(AnchorPhase::Trail(anchors)));
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
            return self.enter_provider_unavailable();
        }

        if finished
            .iter()
            .any(|finished| anchors.contains_producer_turn(&finished.call))
        {
            self.phase = Some(AnchorPhase::Trail(anchors));
            return DecisionAnchorTransition::Unchanged;
        }

        if finished.iter().all(|finished| {
            finished.name == GraphCorrelationToolV1::GetCodeSnippet.public_name()
                && finished.call.admission_checked
                && finished.call.admitted_root.is_none()
        }) {
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
                _ => self.enter_incomplete_enabled(SourceEvidence::default()),
            };
        }
        self.phase = phase;
        DecisionAnchorTransition::Unchanged
    }

    fn enter_recovery(&mut self, anchors: AnchorForest, attempts: u8) -> DecisionAnchorTransition {
        if attempts >= MAX_DECISION_ANCHOR_RECOVERY_ATTEMPTS {
            self.enter_incomplete_enabled(anchors.active_evidence())
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
        GraphExplorationClosedV1::recoverable_with_actions(
            recovery
                .anchors
                .missing_kinds(&recovery.active_root, recovery.route),
            recovery.remaining,
            active.evidence.compatible_actions(active, recovery.route),
        )
    }

    fn recovery_denial_details(&self) -> Option<GraphExplorationClosedV1> {
        let AnchorPhase::GapRecovery(recovery) = self.phase.as_ref()? else {
            return None;
        };
        recovery.anchors.roots.get(&recovery.active_root)?;
        GraphExplorationClosedV1::recoverable_without_actions(
            recovery
                .anchors
                .missing_kinds(&recovery.active_root, recovery.route),
            recovery.remaining,
        )
    }

    fn graph_exploration_denial(&self) -> ToolCallDenial {
        let details = match self.exploration {
            ExplorationStatus::EnabledComplete => Some(GraphExplorationClosedV1::completed()),
            ExplorationStatus::GapRecovery => self.recovery_denial_details().or_else(|| {
                let AnchorPhase::GapRecovery(recovery) = self.phase.as_ref()? else {
                    return None;
                };
                recovery.anchors.roots.get(&recovery.active_root)?;
                GraphExplorationClosedV1::exhausted(
                    recovery
                        .anchors
                        .missing_kinds(&recovery.active_root, recovery.route),
                )
            }),
            ExplorationStatus::EnabledIncomplete => self.phase.as_ref().and_then(|phase| {
                let AnchorPhase::EnabledIncomplete(evidence) = phase else {
                    return None;
                };
                GraphExplorationClosedV1::exhausted(evidence.all_missing_kinds())
            }),
            ExplorationStatus::ProviderUnavailable => {
                Some(GraphExplorationClosedV1::conventional_fallback())
            }
            ExplorationStatus::Open => None,
        };
        ToolCallDenial::GraphExplorationClosed(details)
    }
}
