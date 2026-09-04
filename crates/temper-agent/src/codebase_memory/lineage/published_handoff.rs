//! Publication-time authorization for the one active-root selector handoff.

use serde_json::Value;
use temper_agent_core::{LineageAdmissionOutcome, LineageAdmissionStatus};
use temper_protocol_activity::{
    DecisionAnchorTargetKindV1, DecisionEvidenceKindV1, GraphCorrelationToolV1,
    GraphRecoveryActionV1, GraphRecoveryEvidenceKindV1, GraphRecoveryReferenceDispositionV1,
};

use super::{
    DecisionAnchorLineageRegistry, ExpandedRecoverySelector, RECOVERY_SELECTOR_REFERENCE_PREFIX,
    raw_selector::ExactRawSelectorError,
};

#[derive(Clone)]
pub(super) struct PublishedRecoveryHandoff {
    pub(super) root_binding: String,
    pub(super) action: GraphRecoveryActionV1,
    pub(super) reference: String,
}

pub(super) struct CanonicalPublishedRawSelector {
    pub(super) arguments: Value,
    pub(super) evidence_kind: Option<DecisionEvidenceKindV1>,
}

impl DecisionAnchorLineageRegistry {
    pub(super) fn published_reference_disposition(
        &self,
        tool_name: &str,
        arguments: &Value,
        active_root: Option<&str>,
        evidence_kind: Option<DecisionEvidenceKindV1>,
    ) -> Option<GraphRecoveryReferenceDispositionV1> {
        let references = arguments
            .as_object()?
            .values()
            .filter_map(Value::as_str)
            .filter(|value| value.starts_with(RECOVERY_SELECTOR_REFERENCE_PREFIX))
            .collect::<Vec<_>>();
        if references.is_empty() {
            return None;
        }
        let handoff = self
            .published_handoff
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let recognized = references.len() == 1
            && handoff.as_ref().is_some_and(|handoff| {
                handoff.action.is_valid()
                    && handoff.action.tool.public_name() == tool_name
                    && active_root.is_none_or(|root| root == handoff.root_binding)
                    && references[0] == handoff.reference
                    && action_matches(handoff.action, &handoff.reference, arguments, evidence_kind)
            });
        Some(if recognized {
            GraphRecoveryReferenceDispositionV1::Recognized
        } else {
            GraphRecoveryReferenceDispositionV1::Rejected
        })
    }

    /// Infers the source purpose only for the exact opaque source handoff
    /// currently published for the selected root. Raw, sibling, stale, and
    /// structurally incompatible calls remain ineligible.
    pub(super) fn published_source_evidence_kind(
        &self,
        tool_name: &str,
        arguments: &Value,
        active_root: Option<&str>,
        evidence_kind: Option<DecisionEvidenceKindV1>,
    ) -> Option<DecisionEvidenceKindV1> {
        let references = arguments
            .as_object()?
            .values()
            .filter_map(Value::as_str)
            .filter(|value| value.starts_with(RECOVERY_SELECTOR_REFERENCE_PREFIX))
            .collect::<Vec<_>>();
        let handoff = self
            .published_handoff
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let handoff = handoff.as_ref()?;
        let kind = match handoff.action.evidence_kind {
            GraphRecoveryEvidenceKindV1::Implementation => DecisionEvidenceKindV1::Implementation,
            GraphRecoveryEvidenceKindV1::Caller => DecisionEvidenceKindV1::Caller,
            GraphRecoveryEvidenceKindV1::FocusedTest if !handoff.action.include_tests => {
                DecisionEvidenceKindV1::FocusedTest
            }
            GraphRecoveryEvidenceKindV1::Trace | GraphRecoveryEvidenceKindV1::FocusedTest => {
                return None;
            }
        };
        (references.len() == 1
            && handoff.action.is_valid()
            && handoff.action.tool == GraphCorrelationToolV1::GetCodeSnippet
            && handoff.action.selector_kind == DecisionAnchorTargetKindV1::QualifiedName
            && handoff.action.tool.public_name() == tool_name
            && active_root.is_none_or(|root| root == handoff.root_binding)
            && references[0] == handoff.reference
            && action_matches(handoff.action, &handoff.reference, arguments, evidence_kind))
        .then_some(kind)
    }

    pub(super) fn published_source_arguments(
        &self,
        tool_name: &str,
        arguments: &Value,
        active_root: Option<&str>,
    ) -> Option<Value> {
        let kind = self.published_source_evidence_kind(tool_name, arguments, active_root, None)?;
        let mut inferred = arguments.clone();
        inferred["decision_evidence_kind"] =
            serde_json::to_value(kind).expect("closed evidence kind serializes");
        Some(inferred)
    }

    /// Replaces an exact provider-returned selector with its opaque identity
    /// only inside the wrapper invocation. The raw value is never projected
    /// into diagnostics or durable lineage.
    pub(super) fn canonical_published_raw_selector(
        &self,
        tool_name: &str,
        arguments: &Value,
        active_root: Option<&str>,
        evidence_kind: Option<DecisionEvidenceKindV1>,
    ) -> Result<Option<CanonicalPublishedRawSelector>, ()> {
        if self
            .published_handoff
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_none()
        {
            return Ok(None);
        }
        let lineages = self
            .lineages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let selected = match lineages.exact_raw_selector(tool_name, arguments, evidence_kind, true)
        {
            Ok(Some(selected)) => selected,
            Ok(None) | Err(ExactRawSelectorError::ActiveRootTraceFallback) => return Ok(None),
            Err(ExactRawSelectorError::Invalid) => return Err(()),
        };
        if !selected.action.is_valid()
            || active_root.is_some_and(|root| root != selected.root_binding)
        {
            return Err(());
        }
        Ok(Some(CanonicalPublishedRawSelector {
            arguments: selected.canonical_arguments(arguments),
            evidence_kind: selected.evidence_kind,
        }))
    }

    pub(super) fn resolve_published_raw_admission(
        &self,
        tool_name: &str,
        arguments: &Value,
        active_root: Option<&str>,
    ) -> Option<(
        LineageAdmissionOutcome,
        Option<GraphRecoveryReferenceDispositionV1>,
    )> {
        let selected = {
            let lineages = self
                .lineages
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match lineages.exact_raw_selector(tool_name, arguments, None, false) {
                Ok(Some(selected)) => selected,
                Ok(None) | Err(ExactRawSelectorError::ActiveRootTraceFallback) => return None,
                Err(ExactRawSelectorError::Invalid) => {
                    return Some((
                        LineageAdmissionOutcome::Ineligible(
                            LineageAdmissionStatus::MalformedSelector,
                        ),
                        Some(GraphRecoveryReferenceDispositionV1::Rejected),
                    ));
                }
            }
        };
        let selects_different_root = active_root != Some(selected.root_binding.as_str());
        if !selects_different_root
            && !selected.reference_required
            && arguments.get("decision_evidence_kind").is_some()
        {
            return None;
        }
        let canonical = selected.canonical_arguments(arguments);
        let outcome = self
            .lineages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .resolve_for_active_root_with_recovery(
                tool_name,
                &canonical,
                Some(&selected.root_binding),
            )
            .0;
        match outcome {
            LineageAdmissionOutcome::Eligible(admission) => {
                *self
                    .published_handoff
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                    Some(PublishedRecoveryHandoff {
                        root_binding: selected.root_binding,
                        action: selected.action,
                        reference: selected.reference,
                    });
                let admission = if selects_different_root {
                    admission.with_forest_root_selection()
                } else {
                    admission
                };
                Some((LineageAdmissionOutcome::Eligible(admission), None))
            }
            LineageAdmissionOutcome::Ineligible(status) => Some((
                LineageAdmissionOutcome::Ineligible(status),
                Some(GraphRecoveryReferenceDispositionV1::Rejected),
            )),
        }
    }

    pub(super) fn clear_completed_handoff(&self, expanded: &ExpandedRecoverySelector) {
        let mut handoff = self
            .published_handoff
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if handoff
            .as_ref()
            .is_some_and(|handoff| handoff.reference == expanded.reference)
        {
            *handoff = None;
        }
    }
}

fn action_matches(
    action: GraphRecoveryActionV1,
    reference: &str,
    arguments: &Value,
    evidence_kind: Option<DecisionEvidenceKindV1>,
) -> bool {
    let Some(object) = arguments.as_object() else {
        return false;
    };
    let selector_count = [
        "query",
        "name_pattern",
        "qn_pattern",
        "pattern",
        "function_name",
        "qualified_name",
    ]
    .iter()
    .filter(|field| object.contains_key(**field))
    .count();
    if selector_count != 1 {
        return false;
    }
    match (action.tool, action.selector_kind) {
        (GraphCorrelationToolV1::GetCodeSnippet, DecisionAnchorTargetKindV1::QualifiedName) => {
            let declared = evidence_kind.or_else(|| {
                object
                    .get("decision_evidence_kind")
                    .cloned()
                    .and_then(|value| serde_json::from_value(value).ok())
            });
            object.get("qualified_name").and_then(Value::as_str) == Some(reference)
                && declared.is_none_or(|kind: DecisionEvidenceKindV1| {
                    matches!(
                        (kind, action.evidence_kind),
                        (
                            DecisionEvidenceKindV1::Implementation,
                            GraphRecoveryEvidenceKindV1::Implementation
                        ) | (
                            DecisionEvidenceKindV1::Caller,
                            GraphRecoveryEvidenceKindV1::Caller
                        ) | (
                            DecisionEvidenceKindV1::FocusedTest,
                            GraphRecoveryEvidenceKindV1::FocusedTest
                        )
                    )
                })
        }
        (GraphCorrelationToolV1::TracePath, DecisionAnchorTargetKindV1::FunctionName) => {
            object.get("function_name").and_then(Value::as_str) == Some(reference)
                && object
                    .get("mode")
                    .and_then(Value::as_str)
                    .is_none_or(|mode| mode == "calls")
                && object
                    .get("direction")
                    .and_then(Value::as_str)
                    .is_none_or(|direction| direction == "inbound")
                && match object.get("include_tests") {
                    Some(include_tests) => include_tests.as_bool() == Some(action.include_tests),
                    None => !action.include_tests,
                }
                && evidence_kind.is_none()
                && !object.contains_key("decision_evidence_kind")
        }
        _ => false,
    }
}
