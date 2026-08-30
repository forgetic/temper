//! Provider-derived actions exposed by one recovery route.

use super::*;

impl SourceEvidence {
    /// Implementation recovery reaches its inbound caller while focused-test
    /// source remains independently admissible on the root that produced it.
    pub(super) fn compatible_actions(
        &self,
        anchor: &Anchor,
        route: RecoveryRoute,
    ) -> BTreeSet<GraphRecoveryActionV1> {
        let mut actions = BTreeSet::new();
        match route {
            RecoveryRoute::Implementation => {
                if !self.has_trace() && self.trace_before_implementation {
                    let action =
                        GraphRecoveryActionV1::for_evidence(GraphRecoveryEvidenceKindV1::Trace);
                    if anchor.supports(action) {
                        actions.insert(action);
                    }
                    return actions;
                }
                if self.needs(DecisionGap::Evidence(
                    DecisionEvidenceKindV1::Implementation,
                )) {
                    let action = GraphRecoveryActionV1::for_evidence(
                        GraphRecoveryEvidenceKindV1::Implementation,
                    );
                    if anchor.supports(action) {
                        actions.insert(action);
                    }
                    if !self.trace_before_implementation {
                        return actions;
                    }
                }
                if !self.has_trace() {
                    let action =
                        GraphRecoveryActionV1::for_evidence(GraphRecoveryEvidenceKindV1::Trace);
                    if anchor.supports(action) {
                        actions.insert(action);
                    }
                    return actions;
                }
                if self.needs(DecisionGap::Evidence(DecisionEvidenceKindV1::Caller))
                    && self.caller_selector_available
                {
                    let action =
                        GraphRecoveryActionV1::for_evidence(GraphRecoveryEvidenceKindV1::Caller);
                    if anchor.supports(action) {
                        actions.insert(action);
                    }
                }
                if self.trace_before_implementation
                    && self.needs(DecisionGap::Evidence(DecisionEvidenceKindV1::FocusedTest))
                    && self.focused_test_selector_available
                {
                    let action = GraphRecoveryActionV1::for_evidence(
                        GraphRecoveryEvidenceKindV1::FocusedTest,
                    );
                    if anchor.supports(action) {
                        actions.insert(action);
                    }
                }
            }
            RecoveryRoute::FocusedTest => {
                if self.needs(DecisionGap::Evidence(DecisionEvidenceKindV1::FocusedTest))
                    && self.focused_test_selector_available
                {
                    let action = GraphRecoveryActionV1::for_evidence(
                        GraphRecoveryEvidenceKindV1::FocusedTest,
                    );
                    if anchor.supports(action) {
                        actions.insert(action);
                    }
                }
            }
        }
        actions
    }
}
