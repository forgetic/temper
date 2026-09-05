//! Bounded source preview lifecycle for presented implementation candidates.

use super::*;

impl DecisionAnchorLineageRegistry {
    pub(super) fn settle_published_recovery_preview(
        &self,
        tool_name: &str,
        arguments: &Value,
        active_root: Option<&str>,
        succeeded: bool,
    ) {
        if tool_name != GraphCorrelationToolV1::GetCodeSnippet.public_name()
            || arguments.get("decision_evidence_kind").is_some()
        {
            return;
        }
        let Some(reference) = arguments.get("qualified_name").and_then(Value::as_str) else {
            return;
        };
        self.lineages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .settle_implementation_preview(active_root, reference, succeeded);
    }
}

impl DecisionAnchorLineages {
    pub(super) fn admit_implementation_preview(
        &mut self,
        root_binding: &str,
        reference: &str,
    ) -> bool {
        let Some(candidate) = self.recovery_reference_selectors.get(reference) else {
            return false;
        };
        let purpose = candidate.purpose;
        let key = RecoverySelectorKey {
            root_binding: root_binding.to_string(),
            purpose,
        };
        if !matches!(
            purpose,
            RecoverySelectorPurpose::ImplementationCandidate
                | RecoverySelectorPurpose::ImplementationCorrection
        ) || !self
            .recovery_references
            .get(&key)
            .is_some_and(|references| references.iter().any(|candidate| candidate == reference))
        {
            return false;
        }
        let Some(candidate) = self.recovery_reference_selectors.get_mut(reference) else {
            return false;
        };
        if candidate.root_binding != root_binding
            || !matches!(
                candidate.purpose,
                RecoverySelectorPurpose::ImplementationCandidate
                    | RecoverySelectorPurpose::ImplementationCorrection
            )
            || !candidate.correction_supported
            || candidate.state != RecoverySelectorState::Available
            || candidate.preview_pending
            || candidate.previewed
            || !candidate.presented
        {
            return false;
        }
        candidate.preview_pending = true;
        true
    }

    pub(super) fn settle_implementation_preview(
        &mut self,
        active_root: Option<&str>,
        reference: &str,
        succeeded: bool,
    ) {
        let Some(candidate) = self.recovery_reference_selectors.get_mut(reference) else {
            return;
        };
        if !candidate.preview_pending
            || active_root.is_some_and(|root| root != candidate.root_binding)
        {
            return;
        }
        candidate.preview_pending = false;
        candidate.previewed = succeeded;
    }

    pub(super) fn implementation_correction_previews_complete(
        &self,
        root_binding: &str,
        references: &[String],
    ) -> bool {
        !references.is_empty()
            && references.iter().all(|reference| {
                self.recovery_reference_selectors
                    .get(reference)
                    .is_some_and(|candidate| {
                        candidate.root_binding == root_binding
                            && candidate.purpose
                                == RecoverySelectorPurpose::ImplementationCorrection
                            && candidate.correction_supported
                            && candidate.presented
                            && (candidate.previewed || candidate.preview_pending)
                            && candidate.state == RecoverySelectorState::Available
                    })
            })
    }

    pub(super) fn implementation_correction_was_previewed(
        &self,
        root_binding: &str,
        reference: &str,
    ) -> bool {
        self.recovery_reference_selectors
            .get(reference)
            .is_some_and(|candidate| {
                candidate.root_binding == root_binding
                    && candidate.purpose == RecoverySelectorPurpose::ImplementationCorrection
                    && candidate.correction_supported
                    && candidate.presented
                    && candidate.previewed
                    && candidate.state == RecoverySelectorState::Available
            })
    }

    pub(super) fn implementation_preview_selector<'a>(
        &'a self,
        tool_name: &str,
        input: &Value,
    ) -> Result<Option<(&'static str, &'a RecoverySelectorReference)>, ()> {
        if GraphCorrelationToolV1::from_public_name(tool_name)
            != Some(GraphCorrelationToolV1::GetCodeSnippet)
        {
            return Err(());
        }
        let object = input.as_object().ok_or(())?;
        let selector_fields = [
            "query",
            "name_pattern",
            "qn_pattern",
            "pattern",
            "function_name",
            "qualified_name",
        ];
        if selector_fields
            .iter()
            .filter(|field| object.contains_key(**field))
            .count()
            != 1
        {
            return Err(());
        }
        let public_reference = object
            .get("qualified_name")
            .and_then(Value::as_str)
            .filter(|value| value.starts_with(RECOVERY_SELECTOR_REFERENCE_PREFIX))
            .ok_or(())?;
        let reference = self
            .recovery_reference_selectors
            .get(public_reference)
            .filter(|reference| {
                matches!(
                    reference.purpose,
                    RecoverySelectorPurpose::ImplementationCandidate
                        | RecoverySelectorPurpose::ImplementationCorrection
                ) && reference.correction_supported
                    && reference.presented
                    && reference.state == RecoverySelectorState::Available
                    && reference.preview_pending
                    && reference
                        .selector(DecisionAnchorTargetKindV1::QualifiedName)
                        .is_some()
            })
            .ok_or(())?;
        Ok(Some(("qualified_name", reference)))
    }
}
