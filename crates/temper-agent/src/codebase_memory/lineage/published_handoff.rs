//! Publication-time authorization for the one active-root selector handoff.

use serde_json::Value;
use temper_agent_core::{LineageAdmissionOutcome, LineageAdmissionStatus};
use temper_protocol_activity::{
    DecisionAnchorTargetKindV1, DecisionEvidenceKindV1, GraphCorrelationToolV1,
    GraphRecoveryActionV1, GraphRecoveryEvidenceKindV1, GraphRecoveryReferenceDispositionV1,
};

use super::{
    DecisionAnchorLineageRegistry, ExpandedRecoverySelector, RECOVERY_SELECTOR_REFERENCE_PREFIX,
};

#[derive(Clone)]
pub(super) struct PublishedRecoveryHandoff {
    pub(super) root_binding: String,
    pub(super) action: GraphRecoveryActionV1,
    pub(super) reference: String,
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

    /// Replaces an exact provider-returned selector with its active opaque
    /// identity only inside the wrapper invocation. The selected value is
    /// never projected into diagnostics or durable lineage.
    pub(super) fn canonical_published_raw_selector(
        &self,
        tool_name: &str,
        arguments: &Value,
        active_root: Option<&str>,
        evidence_kind: Option<DecisionEvidenceKindV1>,
    ) -> Result<Option<Value>, ()> {
        let handoff = self
            .published_handoff
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let Some(handoff) = handoff else {
            return Ok(None);
        };
        let lineages = self
            .lineages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(provider_value) = lineages
            .provider_selector_for_reference(&handoff.reference, handoff.action.selector_kind)
        else {
            return Ok(None);
        };
        if !lineages
            .published_selector_requires_reference(&handoff.reference, handoff.action.selector_kind)
        {
            return Ok(None);
        }
        let selector_fields = [
            "query",
            "name_pattern",
            "qn_pattern",
            "pattern",
            "function_name",
            "qualified_name",
        ];
        let exact_selected = selector_fields
            .iter()
            .any(|field| arguments.get(*field).and_then(Value::as_str) == Some(provider_value));
        let expected_field = action_selector_field(handoff.action).ok_or(())?;
        let requested_selector = arguments.get(expected_field).and_then(Value::as_str);
        if !exact_selected {
            return if requested_selector.is_some_and(|selector| {
                lineages.raw_selector_conflicts_with_published(
                    &handoff.reference,
                    handoff.action,
                    selector,
                )
            }) {
                Err(())
            } else {
                Ok(None)
            };
        }
        if !handoff.action.is_valid()
            || handoff.action.tool.public_name() != tool_name
            || active_root.is_some_and(|root| root != handoff.root_binding)
            || !action_matches(handoff.action, provider_value, arguments, evidence_kind)
            || !lineages.exact_provider_selector_is_unambiguous(
                &handoff.reference,
                handoff.action,
                provider_value,
            )
        {
            return Err(());
        }
        let mut canonical = arguments.clone();
        canonical[expected_field] = Value::String(handoff.reference);
        Ok(Some(canonical))
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
        let canonical =
            match self.canonical_published_raw_selector(tool_name, arguments, active_root, None) {
                Ok(Some(canonical)) => canonical,
                Ok(None) => return None,
                Err(()) => {
                    return Some((
                        LineageAdmissionOutcome::Ineligible(
                            LineageAdmissionStatus::MalformedSelector,
                        ),
                        Some(GraphRecoveryReferenceDispositionV1::Rejected),
                    ));
                }
            };
        let outcome = self
            .lineages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .resolve_for_active_root_with_recovery(tool_name, &canonical, active_root)
            .0;
        Some((outcome, None))
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

fn action_selector_field(action: GraphRecoveryActionV1) -> Option<&'static str> {
    match (action.tool, action.selector_kind) {
        (GraphCorrelationToolV1::GetCodeSnippet, DecisionAnchorTargetKindV1::QualifiedName) => {
            Some("qualified_name")
        }
        (GraphCorrelationToolV1::TracePath, DecisionAnchorTargetKindV1::FunctionName) => {
            Some("function_name")
        }
        _ => None,
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
                && declared.is_some_and(|kind: DecisionEvidenceKindV1| {
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
