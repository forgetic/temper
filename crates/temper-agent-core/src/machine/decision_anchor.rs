//! Privacy-safe, per-run enforcement for codebase-memory decision anchors.
//!
//! The policy never retains provider text, model arguments, paths, source, or
//! target digests. The trusted wrapper resolves provider-shaped selections in
//! process and hands this state only a bounded typed lineage record.

use std::collections::{BTreeMap, BTreeSet};

use temper_protocol_activity::{
    DecisionAnchorLineageStageV1, DecisionAnchorLineageV1, DecisionAnchorTargetKindV1,
    GraphCorrelationToolV1, GraphCorrelationV1,
};
use tongs::model::ToolCall;
use tongs::tools::{ToolEffects, ToolOutput};

use super::protocol::{
    CODEBASE_MEMORY_TOOL_PREFIX, SAFE_GRAPH_CORRELATION_DETAIL_KEY, SAFE_TOOL_FAILURE_DETAIL_KEY,
};
use super::tool_failure::ToolFailureCategory;

/// Reserved wrapper detail carrying a process-local-root-bound lineage record.
/// It is deliberately excluded from durable activity metadata.
pub const SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY: &str = "temper_decision_anchor_lineage_v1";
/// Fixed, model-visible explanation for a locally denied mutation.
pub const DECISION_ANCHOR_MUTATION_BLOCKED_MESSAGE: &str = "workspace mutation blocked until the successful decision anchor is consumed through later result-derived codebase-memory evidence for the implementation, caller/model, and focused behavioral tests";

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

pub(super) struct DecisionAnchorState {
    mutation_tools: BTreeSet<String>,
    phase: Option<AnchorPhase>,
    calls: BTreeMap<String, PendingCodebaseCall>,
}

enum AnchorPhase {
    Root(AnchorForest),
    Trail(Trail),
    Recovery(Recovery),
    Exhausted,
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

#[derive(Default)]
struct SourceEvidence {
    refinement_seen: bool,
    trace_turn: Option<usize>,
    source_reads_after_trace: u8,
}

#[derive(Clone, Copy)]
struct PendingCodebaseCall {
    turn: usize,
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
pub(super) enum DecisionAnchorTransition {
    Unchanged,
    RecoveryNeeded,
    RecoveryExhausted,
}

impl Anchor {
    fn from_output(turn: usize, lineage: &DecisionAnchorLineageV1) -> Self {
        Self {
            produced_turn: turn,
            result_target_kinds: lineage.result_target_kinds.iter().copied().collect(),
        }
    }

    fn is_consumable(&self) -> bool {
        !self.result_target_kinds.is_empty()
    }

    fn accepts(&self, call: &PendingCodebaseCall, lineage: &DecisionAnchorLineageV1) -> bool {
        call.turn > self.produced_turn && self.result_target_kinds.contains(&lineage.target_kind)
    }
}

impl AnchorForest {
    fn from_finished(
        finished: &[FinishedCodebaseCall<'_>],
        after_turn: Option<usize>,
    ) -> Option<Self> {
        let mut roots = BTreeMap::new();
        let mut valid = true;
        let mut latest_produced_turn = 0;
        let mut trace_root_turn: Option<usize> = None;
        let mut saw_root = false;

        for finished in finished {
            let Some(output) = anchor_output(finished.name, finished.output) else {
                continue;
            };
            if output.lineage.stage != DecisionAnchorLineageStageV1::Root
                || after_turn.is_some_and(|turn| finished.call.turn <= turn)
            {
                continue;
            }
            saw_root = true;
            latest_produced_turn = latest_produced_turn.max(finished.call.turn);
            if output.tool == GraphCorrelationToolV1::TracePath {
                trace_root_turn = Some(
                    trace_root_turn.map_or(finished.call.turn, |turn| turn.min(finished.call.turn)),
                );
            }
            if roots.len() >= MAX_DECISION_ANCHOR_ROOTS {
                // Overflowing roots are not partially usable: that would make
                // policy depend on an arbitrary provider-result subset.
                valid = false;
                continue;
            }
            match roots.entry(output.lineage.root_binding.clone()) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(Anchor::from_output(finished.call.turn, &output.lineage));
                }
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    let existing = entry.get_mut();
                    existing.produced_turn = existing.produced_turn.min(finished.call.turn);
                    existing
                        .result_target_kinds
                        .extend(output.lineage.result_target_kinds.iter().copied());
                }
            }
        }

        saw_root.then_some(Self {
            roots,
            valid,
            latest_produced_turn,
            trace_root_turn,
        })
    }

    fn merge(&mut self, next: Self) -> bool {
        let mut added = false;
        self.valid &= next.valid;
        self.latest_produced_turn = self.latest_produced_turn.max(next.latest_produced_turn);
        self.trace_root_turn = match (self.trace_root_turn, next.trace_root_turn) {
            (Some(current), Some(next)) => Some(current.min(next)),
            (Some(turn), None) | (None, Some(turn)) => Some(turn),
            (None, None) => None,
        };
        for (root_binding, next_root) in next.roots {
            let has_capacity = self.roots.len() < MAX_DECISION_ANCHOR_ROOTS;
            match self.roots.entry(root_binding) {
                std::collections::btree_map::Entry::Vacant(entry) if has_capacity => {
                    entry.insert(next_root);
                    added = true;
                }
                std::collections::btree_map::Entry::Vacant(_) => self.valid = false,
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    let existing = entry.get_mut();
                    existing.produced_turn = existing.produced_turn.min(next_root.produced_turn);
                    existing
                        .result_target_kinds
                        .extend(next_root.result_target_kinds);
                }
            }
        }
        added
    }

    fn is_consumable(&self) -> bool {
        self.valid && self.roots.values().any(Anchor::is_consumable)
    }

    fn accepts(&self, call: &PendingCodebaseCall, lineage: &DecisionAnchorLineageV1) -> bool {
        self.valid
            && lineage.stage == DecisionAnchorLineageStageV1::CarryForward
            && self
                .roots
                .get(&lineage.root_binding)
                .is_some_and(|root| root.accepts(call, lineage))
    }

    fn contains_producer_turn(&self, call: &PendingCodebaseCall) -> bool {
        call.turn <= self.latest_produced_turn
    }
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
        (has_codebase_memory && !mutation_tools.is_empty()).then_some(Self {
            mutation_tools,
            phase: None,
            calls: BTreeMap::new(),
        })
    }

    pub(super) fn on_tool_dispatched(&mut self, call: &ToolCall, turn: usize) {
        if !call.name.starts_with(CODEBASE_MEMORY_TOOL_PREFIX) {
            return;
        }
        if let Some(call_key) = GraphCorrelationV1::target_digest(&call.id) {
            self.calls.insert(call_key, PendingCodebaseCall { turn });
        }
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
            // Roots start a new policy phase only after the complete batch
            // settles. Keep all bounded independent roots from that snapshot;
            // no sibling from their producer turn is allowed to consume them.
            return match AnchorForest::from_finished(&finished, None) {
                Some(anchors) => self.install_roots(anchors, 0),
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
                    self.install_roots(anchors, recovery.attempts.saturating_add(1))
                } else {
                    self.advance_batch_or_recover(
                        recovery.anchors,
                        recovery.evidence,
                        &finished,
                        recovery.attempts.saturating_add(1),
                    )
                }
            }
            Some(AnchorPhase::Exhausted) => {
                self.phase = Some(AnchorPhase::Exhausted);
                DecisionAnchorTransition::Unchanged
            }
        }
    }

    fn install_roots(&mut self, anchors: AnchorForest, attempts: u8) -> DecisionAnchorTransition {
        if anchors.is_consumable() {
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
        let batch_root_trace_turn = next_roots.as_ref().and_then(|roots| roots.trace_root_turn);
        let roots_progressed = next_roots.is_some_and(|next| active.merge(next));
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
        let source_reads = trace_turn.map_or(0, |trace_turn| {
            compatible
                .iter()
                .filter(|(call, output)| {
                    output.tool == GraphCorrelationToolV1::GetCodeSnippet && call.turn >= trace_turn
                })
                .count() as u8
        });
        let refinement = !evidence.refinement_seen
            && compatible
                .iter()
                .any(|(_, output)| output.tool == GraphCorrelationToolV1::SearchCode);
        let progressed =
            batch_trace_turn.is_some() || source_reads > 0 || refinement || roots_progressed;

        if progressed {
            let mut evidence = evidence;
            evidence.refinement_seen |= refinement;
            if let Some(trace_turn) = batch_trace_turn {
                evidence.record_trace(trace_turn);
            }
            if batch_trace_turn.is_some() {
                evidence.source_reads_after_trace = 0;
            }
            evidence.record_sources(source_reads);
            self.phase = Some(AnchorPhase::Trail(Trail {
                anchors: active,
                evidence,
            }));
            return DecisionAnchorTransition::Unchanged;
        }

        if finished.iter().any(|finished| {
            trusted_unavailable_provider_output(finished.name, finished.output)
                && !active.contains_producer_turn(&finished.call)
                && evidence.expects(graph_tool_for_name(finished.name), batch_trace_turn)
        }) {
            // The trusted wrapper has already supplied fixed, bounded fallback
            // guidance. Release only an unavailable viable evidence step; an
            // unrelated failed graph discovery call cannot bypass this root.
            self.phase = None;
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
            DecisionAnchorTransition::Unchanged
        } else {
            self.enter_recovery(active, evidence, recovery_attempts)
        }
    }

    fn enter_recovery(
        &mut self,
        anchors: AnchorForest,
        evidence: SourceEvidence,
        attempts: u8,
    ) -> DecisionAnchorTransition {
        if attempts >= MAX_DECISION_ANCHOR_RECOVERY_ATTEMPTS {
            self.phase = Some(AnchorPhase::Exhausted);
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
                AnchorPhase::Recovery(_) | AnchorPhase::Exhausted => true,
            })
    }
}

impl SourceEvidence {
    fn record_trace(&mut self, turn: usize) {
        self.trace_turn = Some(self.trace_turn.map_or(turn, |current| current.min(turn)));
    }

    fn record_sources(&mut self, count: u8) {
        self.source_reads_after_trace = self.source_reads_after_trace.saturating_add(count);
    }

    /// Search-code is an optional refinement. A direct current-root trace is
    /// equally valid, and snippets may share that trace's read-only batch.
    fn expects(
        &self,
        tool: Option<GraphCorrelationToolV1>,
        batch_trace_turn: Option<usize>,
    ) -> bool {
        match tool {
            Some(GraphCorrelationToolV1::SearchCode) => true,
            Some(GraphCorrelationToolV1::TracePath) => !self.has_trace(),
            Some(GraphCorrelationToolV1::GetCodeSnippet) => {
                self.has_trace() || batch_trace_turn.is_some()
            }
            Some(GraphCorrelationToolV1::SearchGraph) | None => false,
        }
    }

    fn has_trace(&self) -> bool {
        self.trace_turn.is_some()
    }

    fn is_complete(&self) -> bool {
        self.trace_turn.is_some() && self.source_reads_after_trace >= 2
    }
}

fn graph_tool_for_name(name: &str) -> Option<GraphCorrelationToolV1> {
    match name {
        "codebase_memory_search_graph" => Some(GraphCorrelationToolV1::SearchGraph),
        "codebase_memory_search_code" => Some(GraphCorrelationToolV1::SearchCode),
        "codebase_memory_trace_path" => Some(GraphCorrelationToolV1::TracePath),
        "codebase_memory_get_code_snippet" => Some(GraphCorrelationToolV1::GetCodeSnippet),
        _ => None,
    }
}

fn trusted_unavailable_provider_output(name: &str, output: &ToolOutput) -> bool {
    if !output.is_error || !name.starts_with(CODEBASE_MEMORY_TOOL_PREFIX) {
        return false;
    }
    let Some(marker) = output
        .details
        .as_ref()
        .and_then(|details| details.get(SAFE_TOOL_FAILURE_DETAIL_KEY))
    else {
        return false;
    };
    if marker.get("source").and_then(serde_json::Value::as_str) != Some("codebase_memory") {
        return false;
    }
    marker
        .get("category")
        .and_then(serde_json::Value::as_str)
        .and_then(ToolFailureCategory::from_stable_str)
        // Exploration closure grants no new authority. In particular, an
        // expected lifecycle denial cannot release an incomplete anchor as if
        // the provider had become systemically unavailable.
        .is_some_and(|category| category != ToolFailureCategory::GraphLifecycleDenial)
}

fn anchor_output(name: &str, output: &ToolOutput) -> Option<AnchorOutput> {
    if output.is_error || !name.starts_with(CODEBASE_MEMORY_TOOL_PREFIX) {
        return None;
    }
    let details = output.details.as_ref()?;
    let correlation: GraphCorrelationV1 =
        serde_json::from_value(details.get(SAFE_GRAPH_CORRELATION_DETAIL_KEY)?.clone()).ok()?;
    if !correlation.is_valid() || correlation.tool.public_name() != name {
        return None;
    }
    let lineage: DecisionAnchorLineageV1 = serde_json::from_value(
        details
            .get(SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY)?
            .clone(),
    )
    .ok()?;
    lineage.is_valid_for(&correlation).then_some(AnchorOutput {
        lineage,
        tool: correlation.tool,
    })
}
