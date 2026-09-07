//! Root-local decision evidence and its closed recovery actions.

use crate::EligibleLineageAdmission;

use super::*;

#[derive(Clone, Default)]
pub(super) struct SourceEvidence {
    pub(super) trace_turn: Option<usize>,
    pub(super) decision_kinds: BTreeSet<DecisionEvidenceKindV1>,
    pub(super) caller_selector_available: bool,
    pub(super) trace_before_implementation: bool,
    pub(super) caller_traversal_outcome: Option<CallerDiscoveryOutcomeV1>,
    pub(super) focused_test_selector_available: bool,
    pub(super) focused_test_traversal_turn: Option<usize>,
    pub(super) focused_test_traversal_outcome: Option<FocusedTestDiscoveryOutcomeV1>,
    pub(super) focused_test_fallback_turn: Option<usize>,
    pub(super) focused_test_fallback_outcome: Option<FocusedTestDiscoveryOutcomeV1>,
    pub(super) implementation_correction_available: bool,
    pub(super) implementation_authority_corrected: bool,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum RecoveryRoute {
    Implementation,
    FocusedTest,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum DecisionGap {
    Trace,
    Evidence(DecisionEvidenceKindV1),
}

impl SourceEvidence {
    pub(super) fn record_trace(&mut self, turn: usize) {
        self.trace_turn = Some(self.trace_turn.map_or(turn, |current| current.min(turn)));
    }

    pub(super) fn record_implementation_correction_availability(&mut self, available: bool) {
        self.implementation_correction_available |= available;
    }

    pub(super) fn retain_provisional_implementation_authority(&mut self) {
        self.implementation_correction_available = false;
    }

    pub(super) fn mark_implementation_authority_corrected(&mut self) {
        self.implementation_correction_available = false;
        self.implementation_authority_corrected = true;
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
        // The mandatory post-caller search must produce the selector used by
        // the focused-test source stage. Initial discovery may over-return a
        // test shape, but it cannot bypass this search.
        self.focused_test_selector_available = false;
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
        self.implementation_correction_available |= other.implementation_correction_available;
        self.implementation_authority_corrected |= other.implementation_authority_corrected;
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
            Some(GraphCorrelationToolV1::GetCodeSnippet) => match gap {
                Some(DecisionGap::Evidence(DecisionEvidenceKindV1::Implementation)) => self.needs(
                    DecisionGap::Evidence(DecisionEvidenceKindV1::Implementation),
                ),
                Some(DecisionGap::Evidence(DecisionEvidenceKindV1::Caller)) => {
                    self.has_trace()
                        && self.needs(DecisionGap::Evidence(DecisionEvidenceKindV1::Caller))
                }
                Some(DecisionGap::Evidence(DecisionEvidenceKindV1::FocusedTest)) => {
                    self.needs(DecisionGap::Evidence(DecisionEvidenceKindV1::FocusedTest))
                }
                Some(DecisionGap::Trace) | None => false,
            },
            Some(GraphCorrelationToolV1::SearchGraph) => {
                gap == Some(DecisionGap::Evidence(DecisionEvidenceKindV1::FocusedTest))
                    && self.needs(DecisionGap::Evidence(DecisionEvidenceKindV1::FocusedTest))
            }
            None => false,
        }
    }

    pub(super) fn needs(&self, gap: DecisionGap) -> bool {
        match gap {
            DecisionGap::Trace => !self.has_trace(),
            DecisionGap::Evidence(DecisionEvidenceKindV1::Caller) => !self.caller_is_complete(),
            DecisionGap::Evidence(kind) => !self.decision_kinds.contains(&kind),
        }
    }

    pub(super) fn has_trace(&self) -> bool {
        self.trace_turn.is_some()
    }

    pub(super) fn progress_count(&self) -> usize {
        usize::from(self.has_trace())
            + self.decision_kinds.len()
            + usize::from(
                self.no_production_callers_reported()
                    && !self
                        .decision_kinds
                        .contains(&DecisionEvidenceKindV1::Caller),
            )
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

    pub(super) fn all_missing_kinds(&self) -> Vec<GraphRecoveryEvidenceKindV1> {
        self.missing_gaps()
            .into_iter()
            .map(DecisionGap::recovery_kind)
            .collect()
    }

    pub(super) fn missing_kinds(&self, route: RecoveryRoute) -> Vec<GraphRecoveryEvidenceKindV1> {
        match route {
            RecoveryRoute::Implementation => {
                let mut missing = Vec::new();
                if self.needs(DecisionGap::Evidence(
                    DecisionEvidenceKindV1::Implementation,
                )) {
                    missing.push(GraphRecoveryEvidenceKindV1::Implementation);
                }
                if !self.has_trace() {
                    missing.push(GraphRecoveryEvidenceKindV1::Trace);
                }
                if self.needs(DecisionGap::Evidence(DecisionEvidenceKindV1::Caller)) {
                    missing.push(GraphRecoveryEvidenceKindV1::Caller);
                }
                missing.sort();
                missing
            }
            RecoveryRoute::FocusedTest => self
                .needs(DecisionGap::Evidence(DecisionEvidenceKindV1::FocusedTest))
                .then_some(GraphRecoveryEvidenceKindV1::FocusedTest)
                .into_iter()
                .collect(),
        }
    }

    pub(super) fn implementation_progress_count(&self) -> usize {
        usize::from(self.has_trace())
            + usize::from(
                self.decision_kinds
                    .contains(&DecisionEvidenceKindV1::Implementation),
            )
            + usize::from(self.caller_is_complete())
    }

    pub(super) fn implementation_is_complete(&self) -> bool {
        self.has_trace()
            && self
                .decision_kinds
                .contains(&DecisionEvidenceKindV1::Implementation)
            && self.caller_is_complete()
    }

    fn caller_is_complete(&self) -> bool {
        self.decision_kinds
            .contains(&DecisionEvidenceKindV1::Caller)
            || self.no_production_callers_reported()
    }

    fn no_production_callers_reported(&self) -> bool {
        self.has_trace()
            && !self.caller_selector_available
            && self.caller_traversal_outcome
                == Some(CallerDiscoveryOutcomeV1::NoProductionCallersReported)
    }

    pub(super) fn focused_test_is_complete(&self) -> bool {
        self.decision_kinds
            .contains(&DecisionEvidenceKindV1::FocusedTest)
    }

    pub(super) fn route_is_complete(&self, route: RecoveryRoute) -> bool {
        match route {
            RecoveryRoute::Implementation => self.implementation_is_complete(),
            RecoveryRoute::FocusedTest => self.focused_test_is_complete(),
        }
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

impl DecisionGap {
    pub(super) fn recovery_kind(self) -> GraphRecoveryEvidenceKindV1 {
        match self {
            Self::Trace => GraphRecoveryEvidenceKindV1::Trace,
            Self::Evidence(DecisionEvidenceKindV1::Implementation) => {
                GraphRecoveryEvidenceKindV1::Implementation
            }
            Self::Evidence(DecisionEvidenceKindV1::Caller) => GraphRecoveryEvidenceKindV1::Caller,
            Self::Evidence(DecisionEvidenceKindV1::FocusedTest) => {
                GraphRecoveryEvidenceKindV1::FocusedTest
            }
        }
    }

    pub(super) fn recovery_action_for_admission(
        admission: &EligibleLineageAdmission,
    ) -> Option<GraphRecoveryActionV1> {
        if admission.is_implementation_authority_correction() {
            return Some(GraphRecoveryActionV1::implementation_authority_correction());
        }
        if admission.recovery_purpose() == Some(DecisionEvidenceKindV1::FocusedTest) {
            return match admission.tool_kind() {
                GraphCorrelationToolV1::TracePath => {
                    Some(GraphRecoveryActionV1::focused_test_traversal())
                }
                GraphCorrelationToolV1::SearchGraph => {
                    Some(GraphRecoveryActionV1::focused_test_semantic_fallback())
                }
                GraphCorrelationToolV1::SearchCode | GraphCorrelationToolV1::GetCodeSnippet => None,
            };
        }
        DecisionGap::from_admission(admission)
            .map(|gap| GraphRecoveryActionV1::for_evidence(gap.recovery_kind()))
    }

    pub(super) fn recovery_action_for_call(call: &ToolCall) -> Option<GraphRecoveryActionV1> {
        if call.name == GraphCorrelationToolV1::SearchGraph.public_name()
            && call.arguments.get("query").is_some()
        {
            return Some(GraphRecoveryActionV1::focused_test_semantic_fallback());
        }
        let gap = DecisionGap::from_call(call)?;
        if call.name == GraphCorrelationToolV1::TracePath.public_name()
            && gap == DecisionGap::Evidence(DecisionEvidenceKindV1::FocusedTest)
        {
            Some(GraphRecoveryActionV1::focused_test_traversal())
        } else {
            Some(GraphRecoveryActionV1::for_evidence(gap.recovery_kind()))
        }
    }
}
