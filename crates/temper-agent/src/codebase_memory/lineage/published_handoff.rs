//! Publication-time authorization for the one active-root selector handoff.

use serde_json::Value;
use temper_agent_core::{
    EligibleLineageAdmission, LineageAdmissionOutcome, LineageAdmissionStatus,
};
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
    pub(super) references: Vec<String>,
    pub(super) selected_reference: Option<String>,
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
                    && handoff
                        .references
                        .iter()
                        .any(|reference| reference == references[0])
                    && handoff
                        .selected_reference
                        .as_ref()
                        .is_none_or(|selected| selected == references[0])
                    && action_matches(handoff.action, references[0], arguments, evidence_kind)
            });
        Some(if recognized {
            GraphRecoveryReferenceDispositionV1::Recognized
        } else {
            GraphRecoveryReferenceDispositionV1::Rejected
        })
    }

    pub(super) fn published_implementation_preview_admission(
        &self,
        tool_name: &str,
        arguments: &Value,
        active_root: Option<&str>,
    ) -> Option<EligibleLineageAdmission> {
        let (root_binding, reference, correction) = {
            let handoff = self
                .published_handoff
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let handoff = handoff.as_ref()?;
            let reference = handoff
                .references
                .iter()
                .find(|reference| {
                    handoff.is_implementation_preview(tool_name, arguments, active_root, None)
                        && action_matches(handoff.action, reference, arguments, None)
                })?
                .clone();
            (
                handoff.root_binding.clone(),
                reference,
                handoff.action == GraphRecoveryActionV1::implementation_authority_correction(),
            )
        };
        self.lineages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .admit_implementation_preview(&root_binding, &reference)
            .then(|| {
                if correction {
                    EligibleLineageAdmission::implementation_authority_correction(
                        root_binding,
                        true,
                    )
                } else {
                    EligibleLineageAdmission::implementation_candidate_preview(root_binding)
                }
            })
            .flatten()
    }

    pub(super) fn published_implementation_correction_admission(
        &self,
        tool_name: &str,
        arguments: &Value,
        active_root: Option<&str>,
    ) -> Option<EligibleLineageAdmission> {
        let root_binding = {
            let handoff = self
                .published_handoff
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let handoff = handoff.as_ref()?;
            (handoff.action == GraphRecoveryActionV1::implementation_authority_correction()
                && handoff.action.tool.public_name() == tool_name
                && active_root.is_none_or(|root| root == handoff.root_binding)
                && arguments.get("decision_evidence_kind").is_some()
                && handoff
                    .references
                    .iter()
                    .any(|reference| action_matches(handoff.action, reference, arguments, None)))
            .then(|| handoff.root_binding.clone())?
        };
        EligibleLineageAdmission::implementation_authority_correction(root_binding, false)
    }

    pub(super) fn is_published_implementation_preview(
        &self,
        tool_name: &str,
        arguments: &Value,
        evidence_kind: Option<DecisionEvidenceKindV1>,
    ) -> bool {
        self.published_handoff
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .is_some_and(|handoff| {
                handoff.is_implementation_preview(tool_name, arguments, None, evidence_kind)
            })
    }

    pub(super) fn select_published_reference(
        &self,
        tool_name: &str,
        arguments: &Value,
        active_root: Option<&str>,
    ) -> bool {
        let Some(reference) = arguments.as_object().and_then(|object| {
            let references = object
                .values()
                .filter_map(Value::as_str)
                .filter(|value| value.starts_with(RECOVERY_SELECTOR_REFERENCE_PREFIX))
                .collect::<Vec<_>>();
            (references.len() == 1).then_some(references[0])
        }) else {
            return false;
        };
        let mut handoff = self
            .published_handoff
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(handoff) = handoff.as_mut() else {
            return false;
        };
        if handoff.selected_reference.is_some()
            || !handoff
                .references
                .iter()
                .any(|candidate| candidate == reference)
            || handoff.action.tool.public_name() != tool_name
            || active_root.is_some_and(|root| root != handoff.root_binding)
            || !action_matches(handoff.action, reference, arguments, None)
        {
            return false;
        }
        handoff.selected_reference = Some(reference.to_string());
        true
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
        if kind == DecisionEvidenceKindV1::Implementation
            && evidence_kind.is_none()
            && arguments.get("decision_evidence_kind").is_none()
        {
            return None;
        }
        (references.len() == 1
            && handoff.action.is_valid()
            && handoff.action.tool == GraphCorrelationToolV1::GetCodeSnippet
            && handoff.action.selector_kind == DecisionAnchorTargetKindV1::QualifiedName
            && handoff.action.tool.public_name() == tool_name
            && active_root.is_none_or(|root| root == handoff.root_binding)
            && handoff
                .references
                .iter()
                .any(|reference| reference == references[0])
            && handoff
                .selected_reference
                .as_ref()
                .is_none_or(|selected| selected == references[0])
            && action_matches(handoff.action, references[0], arguments, evidence_kind))
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
            .as_ref()
            .is_none_or(|handoff| handoff.selected_reference.is_none())
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
        let handoff_guard = self
            .published_handoff
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(published_handoff) = handoff_guard.as_ref() else {
            return None;
        };
        if published_handoff.references.len() > 1
            && arguments.get("decision_evidence_kind").is_none()
        {
            return None;
        }
        drop(handoff_guard);
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
                        references: vec![selected.reference.clone()],
                        selected_reference: Some(selected.reference),
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
            .is_some_and(|handoff| handoff.selected_reference.as_ref() == Some(&expanded.reference))
        {
            *handoff = None;
        }
    }
}

impl PublishedRecoveryHandoff {
    fn is_implementation_preview(
        &self,
        tool_name: &str,
        arguments: &Value,
        active_root: Option<&str>,
        evidence_kind: Option<DecisionEvidenceKindV1>,
    ) -> bool {
        matches!(
            self.action,
            action if action
                == GraphRecoveryActionV1::for_evidence(
                    GraphRecoveryEvidenceKindV1::Implementation,
                )
                || action == GraphRecoveryActionV1::implementation_authority_correction()
        ) && self.action.tool.public_name() == tool_name
            && self.selected_reference.is_none()
            && active_root.is_none_or(|root| root == self.root_binding)
            && evidence_kind.is_none()
            && arguments.get("decision_evidence_kind").is_none()
            && self
                .references
                .iter()
                .any(|reference| action_matches(self.action, reference, arguments, None))
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
            let input_kind = match object.get("decision_evidence_kind") {
                Some(value) => {
                    match serde_json::from_value::<DecisionEvidenceKindV1>(value.clone()) {
                        Ok(kind) => Some(kind),
                        Err(_) => return false,
                    }
                }
                None => None,
            };
            if evidence_kind
                .zip(input_kind)
                .is_some_and(|(inferred, declared)| inferred != declared)
            {
                return false;
            }
            let declared = evidence_kind.or(input_kind);
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
