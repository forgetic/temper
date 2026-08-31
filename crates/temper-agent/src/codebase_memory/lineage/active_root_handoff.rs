//! Active-root selection over run-local opaque recovery references.

use super::*;

impl DecisionAnchorLineages {
    pub(in crate::codebase_memory) fn active_root_recovery_selector(
        &self,
        root_binding: &str,
        action: GraphRecoveryActionV1,
    ) -> Option<&str> {
        let purposes = if action
            == GraphRecoveryActionV1::for_evidence(
                temper_protocol_activity::GraphRecoveryEvidenceKindV1::Implementation,
            ) {
            vec![
                RecoverySelectorPurpose::ImplementationCandidate,
                RecoverySelectorPurpose::ImplementationTrace,
            ]
        } else if action
            == GraphRecoveryActionV1::for_evidence(
                temper_protocol_activity::GraphRecoveryEvidenceKindV1::Trace,
            )
        {
            vec![RecoverySelectorPurpose::ImplementationTrace]
        } else if action
            == GraphRecoveryActionV1::for_evidence(
                temper_protocol_activity::GraphRecoveryEvidenceKindV1::Caller,
            )
        {
            vec![RecoverySelectorPurpose::CallerSource]
        } else if action
            == GraphRecoveryActionV1::for_evidence(
                temper_protocol_activity::GraphRecoveryEvidenceKindV1::FocusedTest,
            )
        {
            vec![RecoverySelectorPurpose::FocusedTestSource]
        } else if action == GraphRecoveryActionV1::focused_test_traversal() {
            vec![RecoverySelectorPurpose::CallerTestTraversal]
        } else {
            return None;
        };
        for purpose in purposes {
            let key = RecoverySelectorKey {
                root_binding: root_binding.to_string(),
                purpose,
            };
            if let Some(reference) = self.recovery_references.get(&key).and_then(|references| {
                references.iter().find(|reference| {
                    self.recovery_reference_selectors
                        .get(*reference)
                        .is_some_and(|selector| {
                            selector.state == RecoverySelectorState::Available
                                && selector.selector(action.selector_kind).is_some()
                                && self.recovery_action_is_admissible(
                                    root_binding,
                                    action,
                                    reference,
                                )
                        })
                })
            }) {
                return Some(reference.as_str());
            }
        }
        None
    }

    fn recovery_action_is_admissible(
        &self,
        root_binding: &str,
        action: GraphRecoveryActionV1,
        reference: &str,
    ) -> bool {
        let Some(arguments) = recovery_action_arguments(action, reference) else {
            return false;
        };
        let mut snapshot = self.clone();
        let (outcome, disposition) = snapshot.resolve_for_active_root_with_recovery(
            action.tool.public_name(),
            &arguments,
            Some(root_binding),
        );
        matches!(
            outcome,
            temper_agent_core::LineageAdmissionOutcome::Eligible(admission)
                if admission.matches_root(root_binding)
        ) && disposition
            == Some(temper_protocol_activity::GraphRecoveryReferenceDispositionV1::Recognized)
    }

    pub(in crate::codebase_memory) fn recovery_reference_disposition(
        &self,
        tool_name: &str,
        input: &Value,
        active_root: Option<&str>,
    ) -> Option<temper_protocol_activity::GraphRecoveryReferenceDispositionV1> {
        let has_reference = input.as_object()?.values().any(|value| {
            value
                .as_str()
                .is_some_and(|value| value.starts_with(RECOVERY_SELECTOR_REFERENCE_PREFIX))
        });
        if !has_reference {
            return None;
        }
        let evidence_kind = input
            .get("decision_evidence_kind")
            .and_then(|value| serde_json::from_value(value.clone()).ok());
        let recognized = self
            .recovery_selector(tool_name, input, evidence_kind)
            .ok()
            .flatten()
            .is_some_and(|(_, reference)| {
                active_root.is_none_or(|active_root| reference.root_binding == active_root)
            });
        Some(if recognized {
            temper_protocol_activity::GraphRecoveryReferenceDispositionV1::Recognized
        } else {
            temper_protocol_activity::GraphRecoveryReferenceDispositionV1::Rejected
        })
    }
}

fn recovery_action_arguments(action: GraphRecoveryActionV1, reference: &str) -> Option<Value> {
    action.is_valid().then_some(())?;
    match (action.tool, action.selector_kind) {
        (GraphCorrelationToolV1::GetCodeSnippet, DecisionAnchorTargetKindV1::QualifiedName) => {
            Some(serde_json::json!({
                "qualified_name": reference,
                "decision_evidence_kind": action.evidence_kind.as_str(),
            }))
        }
        (GraphCorrelationToolV1::TracePath, DecisionAnchorTargetKindV1::FunctionName) => {
            Some(serde_json::json!({
                "function_name": reference,
                "mode": "calls",
                "direction": "inbound",
                "include_tests": action.include_tests,
            }))
        }
        _ => None,
    }
}
