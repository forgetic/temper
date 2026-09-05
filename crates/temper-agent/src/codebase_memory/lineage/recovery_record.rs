//! Root-coherent recording for an expanded opaque recovery selector.

use super::*;
use temper_protocol_activity::GraphRecoveryReferenceDispositionV1;

impl DecisionAnchorLineageRegistry {
    pub(crate) fn recovery_selector_guidance(
        &self,
        lineage: &DecisionAnchorLineageV1,
    ) -> Option<String> {
        self.lineages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .recovery_selector_guidance(&lineage.root_binding)
    }

    pub(crate) fn expand_recovery_selector(
        &self,
        tool_name: &str,
        input: &mut Value,
        evidence_kind: Option<DecisionEvidenceKindV1>,
    ) -> Result<Option<ExpandedRecoverySelector>, ()> {
        let requires_opaque_choice = self
            .published_handoff
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .is_some_and(|handoff| {
                let selector = match (handoff.action.tool, handoff.action.selector_kind) {
                    (
                        GraphCorrelationToolV1::GetCodeSnippet,
                        DecisionAnchorTargetKindV1::QualifiedName,
                    ) => input.get("qualified_name"),
                    (
                        GraphCorrelationToolV1::TracePath,
                        DecisionAnchorTargetKindV1::FunctionName,
                    ) => input.get("function_name"),
                    _ => None,
                };
                handoff.references.len() > 1
                    && handoff.action.tool.public_name() == tool_name
                    && evidence_kind.is_none()
                    && input.get("decision_evidence_kind").is_none()
                    && selector.and_then(Value::as_str).is_some_and(|selector| {
                        !selector.starts_with(RECOVERY_SELECTOR_REFERENCE_PREFIX)
                    })
            });
        if requires_opaque_choice {
            return Err(());
        }
        let mut effective_evidence_kind = evidence_kind;
        let canonicalized_raw = if let Some(canonical) =
            self.canonical_published_raw_selector(tool_name, input, None, evidence_kind)?
        {
            *input = canonical.arguments;
            effective_evidence_kind = canonical.evidence_kind;
            true
        } else {
            false
        };
        if !canonicalized_raw {
            effective_evidence_kind = effective_evidence_kind.or_else(|| {
                self.published_source_evidence_kind(tool_name, input, None, effective_evidence_kind)
            });
        }
        if !canonicalized_raw
            && self
                .published_reference_disposition(tool_name, input, None, effective_evidence_kind)
                .is_some_and(|disposition| {
                    disposition != GraphRecoveryReferenceDispositionV1::Recognized
                })
        {
            return Err(());
        }
        let expanded = self
            .lineages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .expand_recovery_selector(tool_name, input, effective_evidence_kind)?;
        if let Some(object) = input.as_object_mut() {
            object.remove("decision_evidence_kind");
        }
        Ok(expanded.map(|expanded| expanded.with_evidence_kind(effective_evidence_kind)))
    }

    pub(crate) fn complete_candidate_reference(
        &self,
        expanded: &ExpandedRecoverySelector,
        preserve_alternatives: bool,
    ) -> Option<CandidateRecovery> {
        let result = self
            .lineages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .complete_candidate_reference(expanded, preserve_alternatives);
        self.clear_completed_handoff(expanded);
        result
    }
}

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
