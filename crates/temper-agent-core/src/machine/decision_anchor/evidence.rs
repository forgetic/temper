//! Typed evidence completion and bounded gap-recovery lifecycle.

use super::*;

impl DecisionAnchorState {
    pub(super) fn enter_gap_recovery(
        &mut self,
        anchors: AnchorForest,
        evidence: SourceEvidence,
    ) -> DecisionAnchorTransition {
        debug_assert!(!evidence.is_complete());
        if !anchors.is_consumable() {
            self.phase = Some(AnchorPhase::Exhausted);
            self.exploration = ExplorationStatus::BudgetExhausted;
            return DecisionAnchorTransition::RecoveryExhausted;
        }
        self.phase = Some(AnchorPhase::GapRecovery(GapRecovery {
            anchors,
            evidence,
            remaining: MAX_DECISION_GAP_RECOVERY_CALLS,
        }));
        self.exploration = ExplorationStatus::GapRecovery;
        DecisionAnchorTransition::GapRecoveryNeeded
    }

    pub(super) fn advance_gap_recovery(
        &mut self,
        recovery: GapRecovery,
        finished: &[FinishedCodebaseCall<'_>],
    ) -> DecisionAnchorTransition {
        let GapRecovery {
            anchors,
            mut evidence,
            remaining,
        } = recovery;
        let compatible = finished
            .iter()
            .filter_map(|finished| {
                let output = anchor_output(finished.name, finished.output)?;
                anchors
                    .accepts(&finished.call, &output.lineage)
                    .then_some((finished.call, output))
            })
            .collect::<Vec<_>>();
        let batch_trace_turn = compatible
            .iter()
            .filter(|(call, output)| {
                call.recovery_gap == Some(DecisionGap::Trace)
                    && output.tool == GraphCorrelationToolV1::TracePath
            })
            .map(|(call, _)| call.turn)
            .min();
        let trace_turn = evidence.trace_turn.or(batch_trace_turn);
        let decision_kinds = trace_turn.map_or_else(BTreeSet::new, |trace_turn| {
            compatible
                .iter()
                .filter_map(|(call, output)| match call.recovery_gap {
                    Some(DecisionGap::Evidence(expected))
                        if output.tool == GraphCorrelationToolV1::GetCodeSnippet
                            && output.lineage.decision_evidence_kind == Some(expected)
                            && call.turn >= trace_turn =>
                    {
                        Some(expected)
                    }
                    _ => None,
                })
                .collect()
        });
        if let Some(turn) = batch_trace_turn {
            evidence.record_trace(turn);
        }
        evidence.record_decision_kinds(decision_kinds);

        if evidence.is_complete() {
            self.phase = Some(AnchorPhase::Trail(Trail { anchors, evidence }));
            self.exploration = ExplorationStatus::Complete;
            return DecisionAnchorTransition::Converged;
        }

        if finished.iter().any(|finished| {
            trusted_unavailable_provider_output(finished.name, finished.output)
                && finished
                    .call
                    .recovery_gap
                    .is_some_and(|gap| evidence.needs(gap))
        }) {
            self.phase = None;
            self.exploration = ExplorationStatus::BudgetExhausted;
            return DecisionAnchorTransition::Unchanged;
        }

        if remaining == 0 {
            self.phase = Some(AnchorPhase::Exhausted);
            self.exploration = ExplorationStatus::BudgetExhausted;
            return DecisionAnchorTransition::RecoveryExhausted;
        }

        self.phase = Some(AnchorPhase::GapRecovery(GapRecovery {
            anchors,
            evidence,
            remaining,
        }));
        self.exploration = ExplorationStatus::GapRecovery;
        DecisionAnchorTransition::GapRecoveryNeeded
    }
}

impl SourceEvidence {
    pub(super) fn record_trace(&mut self, turn: usize) {
        self.trace_turn = Some(self.trace_turn.map_or(turn, |current| current.min(turn)));
    }

    pub(super) fn record_decision_kinds(
        &mut self,
        kinds: impl IntoIterator<Item = DecisionEvidenceKindV1>,
    ) {
        self.decision_kinds.extend(kinds);
    }

    /// Search-code is an optional refinement. A direct current-root trace is
    /// equally valid, and snippets may share that trace's read-only batch.
    pub(super) fn expects(
        &self,
        gap: Option<DecisionGap>,
        tool: Option<GraphCorrelationToolV1>,
        batch_trace_turn: Option<usize>,
    ) -> bool {
        match tool {
            Some(GraphCorrelationToolV1::SearchCode) => true,
            Some(GraphCorrelationToolV1::TracePath) => !self.has_trace(),
            Some(GraphCorrelationToolV1::GetCodeSnippet) => matches!(
                gap,
                Some(DecisionGap::Evidence(kind))
                    if self.needs(DecisionGap::Evidence(kind))
                        && (self.has_trace() || batch_trace_turn.is_some())
            ),
            Some(GraphCorrelationToolV1::SearchGraph) | None => false,
        }
    }

    pub(super) fn needs(&self, gap: DecisionGap) -> bool {
        match gap {
            DecisionGap::Trace => !self.has_trace(),
            DecisionGap::Evidence(kind) => !self.decision_kinds.contains(&kind),
        }
    }

    pub(super) fn has_trace(&self) -> bool {
        self.trace_turn.is_some()
    }

    pub(super) fn is_complete(&self) -> bool {
        self.trace_turn.is_some()
            && [
                DecisionEvidenceKindV1::Implementation,
                DecisionEvidenceKindV1::Caller,
                DecisionEvidenceKindV1::FocusedTest,
            ]
            .into_iter()
            .all(|kind| self.decision_kinds.contains(&kind))
    }
}

impl DecisionGap {
    pub(super) fn from_call(call: &ToolCall) -> Option<Self> {
        match call.name.as_str() {
            "codebase_memory_trace_path" => Some(Self::Trace),
            "codebase_memory_get_code_snippet" => call
                .arguments
                .get("decision_evidence_kind")
                .cloned()
                .and_then(|value| serde_json::from_value(value).ok())
                .map(Self::Evidence),
            _ => None,
        }
    }
}
