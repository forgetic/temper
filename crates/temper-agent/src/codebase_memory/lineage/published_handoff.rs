//! Publication-time authorization for the one active-root selector handoff.

use serde_json::Value;
use temper_protocol_activity::{
    DecisionAnchorTargetKindV1, DecisionEvidenceKindV1, GraphCorrelationToolV1,
    GraphRecoveryActionV1, GraphRecoveryEvidenceKindV1, GraphRecoveryReferenceDispositionV1,
};

use super::{
    DecisionAnchorLineageRegistry, ExpandedRecoverySelector, RECOVERY_SELECTOR_REFERENCE_PREFIX,
};

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
