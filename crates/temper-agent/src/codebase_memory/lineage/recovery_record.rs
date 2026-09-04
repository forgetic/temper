//! Root-coherent recording for an expanded opaque recovery selector.

use super::*;

/// Transient handle retained only while one wrapper invocation is in flight.
/// It is deliberately not serializable or debug-visible.
pub(in crate::codebase_memory) struct ExpandedRecoverySelector {
    pub(super) reference: String,
    pub(super) root_binding: String,
    pub(super) decision_evidence_kind: Option<DecisionEvidenceKindV1>,
}

impl ExpandedRecoverySelector {
    pub(super) fn root_binding(&self) -> &str {
        &self.root_binding
    }

    pub(super) fn with_evidence_kind(
        mut self,
        decision_evidence_kind: Option<DecisionEvidenceKindV1>,
    ) -> Self {
        self.decision_evidence_kind = decision_evidence_kind;
        self
    }

    pub(in crate::codebase_memory) fn evidence_kind(&self) -> Option<DecisionEvidenceKindV1> {
        self.decision_evidence_kind
    }
}

impl DecisionAnchorLineages {
    pub(super) fn record_with_expanded_recovery(
        &mut self,
        correlation: &GraphCorrelationV1,
        input: &Value,
        typed_parts: Option<&[McpToolResultPart]>,
        decision_evidence_kind: Option<DecisionEvidenceKindV1>,
        expanded: Option<&ExpandedRecoverySelector>,
    ) -> Option<DecisionAnchorLineageV1> {
        self.record_with_recovery_root(
            correlation,
            input,
            typed_parts,
            decision_evidence_kind,
            expanded.map(ExpandedRecoverySelector::root_binding),
        )
    }
}
