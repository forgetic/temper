//! Opaque provider-derived selector references used only inside one agent run.

use super::*;

pub(super) const RECOVERY_SELECTOR_REFERENCE_PREFIX: &str = "temper-recovery-selector:";

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum RecoverySelectorPurpose {
    ImplementationTrace,
    CallerSource,
    CallerTestTraversal,
    FocusedTestSource,
}

impl RecoverySelectorPurpose {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::ImplementationTrace => "implementation_evidence_result",
            Self::CallerSource => "caller_traversal_result",
            Self::CallerTestTraversal => "caller_evidence_result",
            Self::FocusedTestSource => "focused_test_result",
        }
    }

    pub(super) const fn selector_kind(self) -> DecisionAnchorTargetKindV1 {
        match self {
            Self::ImplementationTrace | Self::CallerTestTraversal => {
                DecisionAnchorTargetKindV1::FunctionName
            }
            Self::CallerSource | Self::FocusedTestSource => {
                DecisionAnchorTargetKindV1::QualifiedName
            }
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct RecoverySelectorKey {
    pub(super) root_binding: String,
    pub(super) purpose: RecoverySelectorPurpose,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RecoverySelectorReference {
    pub(super) purpose: RecoverySelectorPurpose,
    pub(super) selector: Selector,
    pub(super) provider_value: String,
}

impl DecisionAnchorLineages {
    pub(in crate::codebase_memory) fn recovery_selector_guidance(
        &self,
        root_binding: &str,
    ) -> Option<String> {
        let references = self
            .recovery_references
            .iter()
            .filter(|(key, _)| key.root_binding == root_binding)
            .map(|(key, reference)| format!("{}={reference}", key.purpose.label()))
            .collect::<Vec<_>>();
        (!references.is_empty()).then(|| {
            format!(
                "[Recovery selector references: {}. Copy a reference exactly into the matching selector field named by Decision guidance; references are run-local, provider-derived, and current-root bound.]",
                references.join(", "),
            )
        })
    }

    pub(in crate::codebase_memory) fn expand_recovery_selector(
        &self,
        tool_name: &str,
        input: &mut Value,
        evidence_kind: Option<DecisionEvidenceKindV1>,
    ) -> Result<(), ()> {
        let Some((field, reference)) = self.recovery_selector(tool_name, input, evidence_kind)?
        else {
            return Ok(());
        };
        input[field] = Value::String(reference.provider_value.clone());
        Ok(())
    }

    pub(super) fn validate_recovery_selector(
        &self,
        tool_name: &str,
        input: &Value,
        evidence_kind: Option<DecisionEvidenceKindV1>,
    ) -> Result<(), ()> {
        self.recovery_selector(tool_name, input, evidence_kind)
            .map(|_| ())
    }

    fn recovery_selector<'a>(
        &'a self,
        tool_name: &str,
        input: &Value,
        evidence_kind: Option<DecisionEvidenceKindV1>,
    ) -> Result<Option<(&'static str, &'a RecoverySelectorReference)>, ()> {
        let Some(tool) = GraphCorrelationToolV1::from_public_name(tool_name) else {
            return Ok(None);
        };
        let field = match tool {
            GraphCorrelationToolV1::SearchGraph => "query",
            GraphCorrelationToolV1::SearchCode => "pattern",
            GraphCorrelationToolV1::TracePath => "function_name",
            GraphCorrelationToolV1::GetCodeSnippet => "qualified_name",
        };
        let Some(reference) = input.get(field).and_then(Value::as_str) else {
            return Ok(None);
        };
        if !reference.starts_with(RECOVERY_SELECTOR_REFERENCE_PREFIX) {
            return Ok(None);
        }
        let expected_purpose = match tool {
            GraphCorrelationToolV1::TracePath => match input.get("include_tests") {
                Some(Value::Bool(true)) => RecoverySelectorPurpose::CallerTestTraversal,
                Some(Value::Bool(false)) | None => RecoverySelectorPurpose::ImplementationTrace,
                Some(_) => return Err(()),
            },
            GraphCorrelationToolV1::GetCodeSnippet => match evidence_kind {
                Some(DecisionEvidenceKindV1::Caller) => RecoverySelectorPurpose::CallerSource,
                Some(DecisionEvidenceKindV1::FocusedTest) => {
                    RecoverySelectorPurpose::FocusedTestSource
                }
                Some(DecisionEvidenceKindV1::Implementation) | None => return Err(()),
            },
            GraphCorrelationToolV1::SearchGraph | GraphCorrelationToolV1::SearchCode => {
                return Err(());
            }
        };
        self.recovery_reference_selectors
            .get(reference)
            .filter(|reference| {
                reference.purpose == expected_purpose
                    && reference.selector.kind == expected_purpose.selector_kind()
            })
            .map(|reference| Some((field, reference)))
            .ok_or(())
    }
}
