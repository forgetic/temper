//! Typed evidence completion and bounded gap-recovery lifecycle.

use crate::EligibleLineageAdmission;

use super::*;

impl DecisionAnchorState {
    pub(super) fn enter_gap_recovery(&mut self, anchors: AnchorForest) -> DecisionAnchorTransition {
        debug_assert!(!anchors.has_complete_evidence());
        let mut anchors = anchors;
        let Some(active_root) = anchors.recovery_root_binding() else {
            self.phase = Some(AnchorPhase::Exhausted(anchors.active_evidence()));
            self.exploration = ExplorationStatus::BudgetExhausted;
            return DecisionAnchorTransition::RecoveryExhausted;
        };
        if anchors.roots.len() > 1 {
            let active = anchors
                .roots
                .get_mut(&active_root)
                .expect("the selected recovery root remains installed");
            active.evidence.trace_before_implementation = true;
        }
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
                .then_some((
                    finished.id,
                    finished.call.clone(),
                    output,
                    finished.source_target,
                ))
            })
            .collect::<Vec<_>>();
        let had_trace = active.evidence.has_trace();
        let had_caller_selector = active.evidence.caller_selector_available;
        let had_focused_test_selector = active.evidence.focused_test_selector_available;
        let batch_trace = compatible
            .iter()
            .filter(|(_, call, output, _)| {
                call.recovery_gap == Some(DecisionGap::Trace)
                    && output.tool == GraphCorrelationToolV1::TracePath
                    && output.lineage.caller_discovery.is_some()
            })
            .min_by_key(|(_, call, _, _)| call.turn)
            .map(|(id, call, output, _)| (*id, call.turn, output.lineage.caller_discovery));
        let decision_kinds = compatible
            .iter()
            .filter_map(|(id, call, output, _)| match call.recovery_gap {
                Some(DecisionGap::Evidence(expected))
                    if output.tool == GraphCorrelationToolV1::GetCodeSnippet
                        && output.lineage.decision_evidence_kind == Some(expected)
                        && match expected {
                            DecisionEvidenceKindV1::Implementation => true,
                            DecisionEvidenceKindV1::Caller => {
                                had_trace && (had_caller_selector || !call.admission_checked)
                            }
                            DecisionEvidenceKindV1::FocusedTest => {
                                had_trace && (had_focused_test_selector || !call.admission_checked)
                            }
                        } =>
                {
                    Some((*id, expected))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let accepted_kinds = decision_kinds
            .iter()
            .map(|(_, kind)| *kind)
            .collect::<BTreeSet<_>>();
        let accepted_sources = compatible
            .iter()
            .filter_map(|(id, call, output, source_target)| {
                let expected = output.lineage.decision_evidence_kind?;
                (output.tool == GraphCorrelationToolV1::GetCodeSnippet
                    && decision_kinds.contains(&(*id, expected)))
                .then_some((*id, call, expected, *source_target))
            })
            .collect::<Vec<_>>();
        for (id, call, expected, source_target) in accepted_sources {
            self.record_source_authority(&active_root, call, source_target);
            self.mark_accepted(id, AcceptedEvidence::from(expected));
        }
        let active = anchors
            .roots
            .get_mut(&active_root)
            .expect("the active recovery root remains installed");
        if let Some((id, turn, outcome)) = batch_trace {
            active.evidence.record_trace(turn);
            active.evidence.record_caller_discovery(outcome);
            self.mark_accepted(id, AcceptedEvidence::Trace);
        }
        for (id, call, output, _) in &compatible {
            if call.recovery_gap != Some(DecisionGap::Evidence(DecisionEvidenceKindV1::FocusedTest))
            {
                continue;
            }
            match output.tool {
                GraphCorrelationToolV1::TracePath => {
                    active.evidence.record_focused_test_traversal(call.turn)
                }
                GraphCorrelationToolV1::SearchGraph => {
                    active.evidence.record_focused_test_fallback(call.turn)
                }
                GraphCorrelationToolV1::SearchCode | GraphCorrelationToolV1::GetCodeSnippet => {
                    continue;
                }
            }
            self.mark_accepted(id, AcceptedEvidence::FocusedTestRoute);
            active
                .evidence
                .record_focused_test_discovery(output.tool, output.lineage.focused_test_discovery);
        }
        active.evidence.record_decision_kinds(accepted_kinds);

        if active.evidence.is_complete() {
            self.phase = Some(AnchorPhase::AwaitingExactRead(anchors));
            self.exploration = ExplorationStatus::Complete;
            return DecisionAnchorTransition::Converged;
        }

        if compatible.iter().any(|(_, call, output, _)| {
            call.recovery_gap == Some(DecisionGap::Evidence(DecisionEvidenceKindV1::FocusedTest))
                && output.tool == GraphCorrelationToolV1::GetCodeSnippet
                && output.lineage.decision_evidence_kind
                    != Some(DecisionEvidenceKindV1::FocusedTest)
        }) {
            self.phase = Some(AnchorPhase::Exhausted(active.evidence.clone()));
            self.exploration = ExplorationStatus::BudgetExhausted;
            return DecisionAnchorTransition::RecoveryExhausted;
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

    pub(super) fn record_caller_discovery(&mut self, outcome: Option<CallerDiscoveryOutcomeV1>) {
        let Some(outcome) = outcome else {
            return;
        };
        if outcome == CallerDiscoveryOutcomeV1::EligibleSelectorReturned {
            self.caller_selector_available = true;
        }
        self.caller_traversal_outcome.get_or_insert(outcome);
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

    pub(super) fn record_focused_test_fallback(&mut self, turn: usize) {
        self.focused_test_fallback_turn = Some(
            self.focused_test_fallback_turn
                .map_or(turn, |current| current.min(turn)),
        );
    }

    pub(super) fn record_focused_test_discovery(
        &mut self,
        tool: GraphCorrelationToolV1,
        outcome: Option<FocusedTestDiscoveryOutcomeV1>,
    ) {
        let Some(outcome) = outcome else {
            return;
        };
        if outcome == FocusedTestDiscoveryOutcomeV1::EligibleSelectorReturned {
            self.focused_test_selector_available = true;
        }
        match tool {
            GraphCorrelationToolV1::TracePath => {
                self.focused_test_traversal_outcome.get_or_insert(outcome);
            }
            GraphCorrelationToolV1::SearchGraph if self.focused_test_fallback_turn.is_some() => {
                self.focused_test_fallback_outcome.get_or_insert(outcome);
            }
            GraphCorrelationToolV1::SearchGraph
            | GraphCorrelationToolV1::SearchCode
            | GraphCorrelationToolV1::GetCodeSnippet => {}
        }
    }

    pub(super) fn merge(&mut self, other: Self) {
        if let Some(turn) = other.trace_turn {
            self.record_trace(turn);
        }
        self.trace_before_implementation |= other.trace_before_implementation;
        self.caller_selector_available |= other.caller_selector_available;
        if self.caller_traversal_outcome.is_none() {
            self.caller_traversal_outcome = other.caller_traversal_outcome;
        }
        if let Some(turn) = other.focused_test_traversal_turn {
            self.record_focused_test_traversal(turn);
        }
        if let Some(turn) = other.focused_test_fallback_turn {
            self.record_focused_test_fallback(turn);
        }
        self.focused_test_selector_available |= other.focused_test_selector_available;
        if self.focused_test_traversal_outcome.is_none() {
            self.focused_test_traversal_outcome = other.focused_test_traversal_outcome;
        }
        if self.focused_test_fallback_outcome.is_none() {
            self.focused_test_fallback_outcome = other.focused_test_fallback_outcome;
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
            Some(GraphCorrelationToolV1::SearchGraph) => {
                gap == Some(DecisionGap::Evidence(DecisionEvidenceKindV1::FocusedTest))
                    && self.focused_test_fallback_turn.is_some()
            }
            None => false,
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
            + usize::from(self.focused_test_fallback_turn.is_some())
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

    /// Recovery advances through exact implementation source, its typed caller
    /// traversal, the exact returned caller source, and then focused-test routes.
    pub(super) fn compatible_actions(&self, anchor: &Anchor) -> BTreeSet<GraphRecoveryActionV1> {
        if !self.has_trace() && self.trace_before_implementation {
            let action = GraphRecoveryActionV1::for_evidence(GraphRecoveryEvidenceKindV1::Trace);
            return anchor
                .supports(action)
                .then_some(action)
                .into_iter()
                .collect();
        }
        let mut actions = BTreeSet::new();
        if self.needs(DecisionGap::Evidence(
            DecisionEvidenceKindV1::Implementation,
        )) {
            let action =
                GraphRecoveryActionV1::for_evidence(GraphRecoveryEvidenceKindV1::Implementation);
            if anchor.supports(action) {
                actions.insert(action);
            }
            if !self.trace_before_implementation {
                return actions;
            }
        }
        if !self.has_trace() {
            let action = GraphRecoveryActionV1::for_evidence(GraphRecoveryEvidenceKindV1::Trace);
            return anchor
                .supports(action)
                .then_some(action)
                .into_iter()
                .collect();
        }

        if self.needs(DecisionGap::Evidence(DecisionEvidenceKindV1::Caller))
            && self.caller_selector_available
        {
            let action = GraphRecoveryActionV1::for_evidence(GraphRecoveryEvidenceKindV1::Caller);
            if anchor.supports(action) {
                actions.insert(action);
            }
        }
        if self.needs(DecisionGap::Evidence(DecisionEvidenceKindV1::FocusedTest))
            && self.focused_test_selector_available
        {
            let action =
                GraphRecoveryActionV1::for_evidence(GraphRecoveryEvidenceKindV1::FocusedTest);
            if anchor.supports(action) {
                actions.insert(action);
            }
        }
        if self.needs(DecisionGap::Evidence(DecisionEvidenceKindV1::FocusedTest))
            && self
                .decision_kinds
                .contains(&DecisionEvidenceKindV1::Caller)
        {
            if self.focused_test_traversal_turn.is_none() {
                let traversal = GraphRecoveryActionV1::focused_test_traversal();
                if anchor.supports(traversal) {
                    actions.insert(traversal);
                }
            } else if self.focused_test_traversal_outcome
                == Some(FocusedTestDiscoveryOutcomeV1::NoEligibleSelector)
                && !self.focused_test_selector_available
                && self.focused_test_fallback_turn.is_none()
            {
                let fallback = GraphRecoveryActionV1::focused_test_semantic_fallback();
                if anchor.supports(fallback) {
                    actions.insert(fallback);
                }
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
