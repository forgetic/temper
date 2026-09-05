//! Bounded source preview lifecycle for presented implementation candidates.

use super::*;

impl DecisionAnchorLineages {
    pub(super) fn admit_implementation_preview(
        &mut self,
        root_binding: &str,
        reference: &str,
    ) -> bool {
        let key = RecoverySelectorKey {
            root_binding: root_binding.to_string(),
            purpose: RecoverySelectorPurpose::ImplementationCandidate,
        };
        if !self
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
            || candidate.purpose != RecoverySelectorPurpose::ImplementationCandidate
            || candidate.state != RecoverySelectorState::Available
            || candidate.previewed
            || !candidate.presented
        {
            return false;
        }
        candidate.previewed = true;
        true
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
                reference.purpose == RecoverySelectorPurpose::ImplementationCandidate
                    && reference.presented
                    && reference.state == RecoverySelectorState::Available
                    && reference.previewed
                    && reference
                        .selector(DecisionAnchorTargetKindV1::QualifiedName)
                        .is_some()
            })
            .ok_or(())?;
        Ok(Some(("qualified_name", reference)))
    }
}
