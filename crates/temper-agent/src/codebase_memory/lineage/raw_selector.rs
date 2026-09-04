//! Exact raw-selector matching for an active published handoff.

use temper_protocol_activity::GraphRecoveryEvidenceKindV1;

use super::*;

impl RecoverySelectorPurpose {
    fn supports_action(self, action: GraphRecoveryActionV1) -> bool {
        match action.evidence_kind {
            GraphRecoveryEvidenceKindV1::Implementation => matches!(
                self,
                Self::ImplementationCandidate | Self::ImplementationTrace
            ),
            GraphRecoveryEvidenceKindV1::Trace => self == Self::ImplementationTrace,
            GraphRecoveryEvidenceKindV1::Caller => self == Self::CallerSource,
            GraphRecoveryEvidenceKindV1::FocusedTest if action.include_tests => {
                self == Self::CallerTestTraversal
            }
            GraphRecoveryEvidenceKindV1::FocusedTest => self == Self::FocusedTestSource,
        }
    }
}

impl DecisionAnchorLineages {
    pub(super) fn provider_selector_for_reference(
        &self,
        reference: &str,
        kind: DecisionAnchorTargetKindV1,
    ) -> Option<&str> {
        self.recovery_reference_selectors
            .get(reference)?
            .provider_value(kind)
    }

    pub(super) fn published_selector_requires_reference(
        &self,
        reference: &str,
        kind: DecisionAnchorTargetKindV1,
    ) -> bool {
        let Some(reference) = self.recovery_reference_selectors.get(reference) else {
            return false;
        };
        let Some(selector) = reference.selector(kind) else {
            return false;
        };
        self.root_selectors
            .get(&(reference.root_binding.clone(), selector))
            .is_some_and(|binding| binding.recovery_reference_required)
    }

    pub(super) fn raw_selector_conflicts_with_published(
        &self,
        reference: &str,
        action: GraphRecoveryActionV1,
        raw_selector: &str,
    ) -> bool {
        let canonical = match action.selector_kind {
            DecisionAnchorTargetKindV1::FunctionName => canonical_function_name(raw_selector),
            DecisionAnchorTargetKindV1::QualifiedName => canonical_qualified_name(raw_selector)
                .or_else(|| canonical_function_name(raw_selector)),
            DecisionAnchorTargetKindV1::Pattern
            | DecisionAnchorTargetKindV1::GraphQuery
            | DecisionAnchorTargetKindV1::NamePattern
            | DecisionAnchorTargetKindV1::QualifiedNamePattern => None,
        }
        .map(|value| Selector {
            kind: action.selector_kind,
            value,
        });
        self.recovery_reference_selectors
            .get(reference)
            .and_then(|selected| selected.selector(action.selector_kind))
            .is_some_and(|selected| canonical.as_ref() == Some(&selected))
            || self.recovery_reference_selectors.values().any(|candidate| {
                candidate.purpose.supports_action(action)
                    && candidate.provider_value(action.selector_kind) == Some(raw_selector)
            })
    }

    pub(super) fn exact_provider_selector_is_unambiguous(
        &self,
        reference: &str,
        action: GraphRecoveryActionV1,
        provider_value: &str,
    ) -> bool {
        let Some(selected) = self.recovery_reference_selectors.get(reference) else {
            return false;
        };
        if !selected.presented
            || selected.state == RecoverySelectorState::Consumed
            || !selected.purpose.supports_action(action)
            || selected.provider_value(action.selector_kind) != Some(provider_value)
        {
            return false;
        }
        self.recovery_reference_selectors
            .values()
            .filter(|candidate| {
                candidate.state != RecoverySelectorState::Consumed
                    && candidate.purpose.supports_action(action)
                    && candidate.provider_value(action.selector_kind) == Some(provider_value)
            })
            .filter_map(|candidate| {
                candidate
                    .selector(action.selector_kind)
                    .map(|selector| (candidate.root_binding.clone(), selector))
            })
            .collect::<BTreeSet<_>>()
            .len()
            == 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RAW: &str = "temper-v1-private.src.route.worker_slot";

    fn reference(root: &str) -> RecoverySelectorReference {
        RecoverySelectorReference {
            root_binding: root.to_string(),
            purpose: RecoverySelectorPurpose::ImplementationCandidate,
            selector: Selector {
                kind: DecisionAnchorTargetKindV1::FunctionName,
                value: "worker_slot".to_string(),
            },
            provider_value: "worker_slot".to_string(),
            source_selector: Some(Selector {
                kind: DecisionAnchorTargetKindV1::QualifiedName,
                value: "temper-v1-private::src::route::worker_slot".to_string(),
            }),
            source_provider_value: Some(RAW.to_string()),
            state: RecoverySelectorState::Available,
            presented: true,
        }
    }

    #[test]
    fn exact_provider_selector_is_unique_while_normalized_and_ambiguous_values_fail_closed() {
        let action =
            GraphRecoveryActionV1::for_evidence(GraphRecoveryEvidenceKindV1::Implementation);
        let mut lineages = DecisionAnchorLineages::default();
        lineages
            .recovery_reference_selectors
            .insert("selected".to_string(), reference("active"));

        assert!(lineages.exact_provider_selector_is_unambiguous("selected", action, RAW));
        assert!(lineages.raw_selector_conflicts_with_published(
            "selected",
            action,
            "  temper-v1-private.src.route.worker_slot  "
        ));

        lineages
            .recovery_reference_selectors
            .insert("sibling".to_string(), reference("sibling"));
        assert!(!lineages.exact_provider_selector_is_unambiguous("selected", action, RAW));
        lineages
            .recovery_reference_selectors
            .get_mut("selected")
            .unwrap()
            .state = RecoverySelectorState::Consumed;
        assert!(!lineages.exact_provider_selector_is_unambiguous("selected", action, RAW));
    }
}
