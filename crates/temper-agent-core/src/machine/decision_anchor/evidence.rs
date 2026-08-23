//! Typed evidence completion and bounded gap-recovery lifecycle.

use crate::EligibleLineageAdmission;

use super::*;

impl DecisionAnchorState {
    pub(super) fn enter_gap_recovery(&mut self, anchors: AnchorForest) -> DecisionAnchorTransition {
        debug_assert!(!anchors.has_complete_evidence());
        let Some(active_root) = anchors.recovery_root_binding() else {
            self.phase = Some(AnchorPhase::Exhausted(anchors.active_evidence()));
            self.exploration = ExplorationStatus::BudgetExhausted;
            return DecisionAnchorTransition::RecoveryExhausted;
        };
        self.phase = Some(AnchorPhase::GapRecovery(GapRecovery {
            anchors,
            active_root,
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
            mut anchors,
            active_root,
            remaining,
        } = recovery;
        let Some(active) = anchors.roots.get(&active_root) else {
            self.phase = Some(AnchorPhase::Exhausted(SourceEvidence::default()));
            self.exploration = ExplorationStatus::BudgetExhausted;
            return DecisionAnchorTransition::RecoveryExhausted;
        };
        let compatible = finished
            .iter()
            .filter_map(|finished| {
                if finished.call.admitted_root.as_deref() != Some(active_root.as_str()) {
                    return None;
                }
                let output = anchor_output(finished.name, finished.output)?;
                (output.lineage.root_binding == active_root
                    && active.accepts(&finished.call, &output.lineage))
                .then_some((finished.call.clone(), output))
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
        let focused_test_traversal_turn = compatible
            .iter()
            .filter(|(call, output)| {
                call.recovery_gap
                    == Some(DecisionGap::Evidence(DecisionEvidenceKindV1::FocusedTest))
                    && output.tool == GraphCorrelationToolV1::TracePath
            })
            .map(|(call, _)| call.turn)
            .min();
        let had_trace = active.evidence.has_trace();
        let decision_kinds = if had_trace {
            compatible
                .iter()
                .filter_map(|(call, output)| match call.recovery_gap {
                    Some(DecisionGap::Evidence(expected))
                        if output.tool == GraphCorrelationToolV1::GetCodeSnippet
                            && output.lineage.decision_evidence_kind == Some(expected) =>
                    {
                        Some(expected)
                    }
                    _ => None,
                })
                .collect::<BTreeSet<_>>()
        } else {
            BTreeSet::new()
        };
        let active = anchors
            .roots
            .get_mut(&active_root)
            .expect("the active recovery root remains installed");
        if let Some(turn) = batch_trace_turn {
            active.evidence.record_trace(turn);
        }
        if let Some(turn) = focused_test_traversal_turn {
            active.evidence.record_focused_test_traversal(turn);
        }
        active.evidence.record_decision_kinds(decision_kinds);

        if active.evidence.is_complete() {
            self.phase = Some(AnchorPhase::Trail(anchors));
            self.exploration = ExplorationStatus::Complete;
            return DecisionAnchorTransition::Converged;
        }

        if finished.iter().any(|finished| {
            trusted_unavailable_provider_output(finished.name, finished.output)
                && finished.call.admitted_root.as_deref() == Some(active_root.as_str())
                && finished
                    .call
                    .recovery_gap
                    .is_some_and(|gap| active.evidence.needs(gap))
        }) {
            self.phase = None;
            self.exploration = ExplorationStatus::BudgetExhausted;
            return DecisionAnchorTransition::Unchanged;
        }

        let evidence = active.evidence.clone();
        let has_path = !active.evidence.compatible_actions(active).is_empty();
        if !has_path || remaining == 0 {
            self.phase = Some(AnchorPhase::Exhausted(evidence));
            self.exploration = ExplorationStatus::BudgetExhausted;
            return DecisionAnchorTransition::RecoveryExhausted;
        }

        self.phase = Some(AnchorPhase::GapRecovery(GapRecovery {
            anchors,
            active_root,
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

    pub(super) fn record_focused_test_traversal(&mut self, turn: usize) {
        self.focused_test_traversal_turn = Some(
            self.focused_test_traversal_turn
                .map_or(turn, |current| current.min(turn)),
        );
    }

    pub(super) fn merge(&mut self, other: Self) {
        if let Some(turn) = other.trace_turn {
            self.record_trace(turn);
        }
        if let Some(turn) = other.focused_test_traversal_turn {
            self.record_focused_test_traversal(turn);
        }
        self.record_decision_kinds(other.decision_kinds);
    }

    pub(super) fn expects(
        &self,
        gap: Option<DecisionGap>,
        tool: Option<GraphCorrelationToolV1>,
    ) -> bool {
        match tool {
            Some(GraphCorrelationToolV1::SearchCode) => true,
            Some(GraphCorrelationToolV1::TracePath) => match gap {
                Some(DecisionGap::Evidence(DecisionEvidenceKindV1::FocusedTest)) => {
                    self.needs(DecisionGap::Evidence(DecisionEvidenceKindV1::FocusedTest))
                }
                _ => !self.has_trace(),
            },
            Some(GraphCorrelationToolV1::GetCodeSnippet) => matches!(
                gap,
                Some(DecisionGap::Evidence(kind))
                    if self.needs(DecisionGap::Evidence(kind)) && self.has_trace()
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

    pub(super) fn progress_count(&self) -> usize {
        usize::from(self.has_trace())
            + self.decision_kinds.len()
            + usize::from(self.focused_test_traversal_turn.is_some())
    }

    pub(super) fn missing_gaps(&self) -> BTreeSet<DecisionGap> {
        let mut missing = BTreeSet::new();
        if !self.has_trace() {
            missing.insert(DecisionGap::Trace);
        }
        for kind in [
            DecisionEvidenceKindV1::Implementation,
            DecisionEvidenceKindV1::Caller,
            DecisionEvidenceKindV1::FocusedTest,
        ] {
            if self.needs(DecisionGap::Evidence(kind)) {
                missing.insert(DecisionGap::Evidence(kind));
            }
        }
        missing
    }

    /// Recovery source reads require a trace in the pre-batch snapshot. A
    /// caller-to-test traversal is offered only after caller source evidence,
    /// and its returned test identity must be consumed in a later turn.
    pub(super) fn compatible_actions(&self, anchor: &Anchor) -> BTreeSet<GraphRecoveryActionV1> {
        if !self.has_trace() {
            let action = GraphRecoveryActionV1::for_evidence(GraphRecoveryEvidenceKindV1::Trace);
            return anchor
                .supports(action)
                .then_some(action)
                .into_iter()
                .collect();
        }
        let mut actions = self
            .missing_gaps()
            .into_iter()
            .filter_map(|gap| {
                let action = GraphRecoveryActionV1::for_evidence(gap.recovery_kind());
                anchor.supports(action).then_some(action)
            })
            .collect::<BTreeSet<_>>();
        if self.needs(DecisionGap::Evidence(DecisionEvidenceKindV1::FocusedTest))
            && self
                .decision_kinds
                .contains(&DecisionEvidenceKindV1::Implementation)
            && self
                .decision_kinds
                .contains(&DecisionEvidenceKindV1::Caller)
            && self.focused_test_traversal_turn.is_none()
        {
            let traversal = GraphRecoveryActionV1::focused_test_traversal();
            if anchor.supports(traversal) {
                actions.insert(traversal);
            }
        }
        actions
    }

    pub(super) fn missing_kinds(&self) -> Vec<GraphRecoveryEvidenceKindV1> {
        self.missing_gaps()
            .into_iter()
            .map(DecisionGap::recovery_kind)
            .collect()
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
    pub(super) fn from_admission(admission: &EligibleLineageAdmission) -> Option<Self> {
        if let Some(kind) = admission.recovery_purpose() {
            return Some(Self::Evidence(kind));
        }
        match admission.tool_kind() {
            GraphCorrelationToolV1::TracePath => Some(Self::Trace),
            GraphCorrelationToolV1::GetCodeSnippet => {
                admission.evidence_purpose().map(Self::Evidence)
            }
            GraphCorrelationToolV1::SearchGraph | GraphCorrelationToolV1::SearchCode => None,
        }
    }

    /// Compatibility path for machines composed without codebase-memory. The
    /// production run always supplies a wrapper-owned admission result.
    pub(super) fn from_call(call: &ToolCall) -> Option<Self> {
        match call.name.as_str() {
            "codebase_memory_trace_path"
                if call
                    .arguments
                    .get("include_tests")
                    .and_then(serde_json::Value::as_bool)
                    == Some(true)
                    && call
                        .arguments
                        .get("direction")
                        .and_then(serde_json::Value::as_str)
                        == Some("inbound")
                    && call
                        .arguments
                        .get("mode")
                        .and_then(serde_json::Value::as_str)
                        == Some("calls") =>
            {
                Some(Self::Evidence(DecisionEvidenceKindV1::FocusedTest))
            }
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
