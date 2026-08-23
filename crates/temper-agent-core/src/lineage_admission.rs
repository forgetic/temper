//! Run-local pre-provider resolution for codebase-memory lineage selectors.
//!
//! The implementation lives in the trusted wrapper tier. Core passes a
//! normalized call to it and receives only closed policy facts; raw arguments
//! and provider-shaped registry values never become machine state or protocol.

use std::fmt;
use std::sync::Arc;

use serde_json::Value;
use temper_protocol_activity::{
    DecisionAnchorTargetKindV1, DecisionEvidenceKindV1, GraphCorrelationToolV1,
};

/// Shared process-local resolver installed for one agent run.
pub type LineageAdmissionHandle = Arc<dyn LineageAdmissionResolver>;

/// Wrapper-owned resolver queried synchronously before a graph request is
/// handed to the provider-bound shell.
pub trait LineageAdmissionResolver: Send + Sync {
    fn resolve(&self, tool_name: &str, arguments: &Value) -> LineageAdmissionOutcome;
}

/// Bounded reasons why a call cannot be tied to one exact registered root.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LineageAdmissionStatus {
    UnknownSelector,
    AmbiguousSelector,
    MalformedSelector,
    BroadSelector,
    UnsupportedTool,
    IncapableSelection,
}

/// Closed pre-provider result. Neither variant can retain a raw selector.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LineageAdmissionOutcome {
    Eligible(EligibleLineageAdmission),
    Ineligible(LineageAdmissionStatus),
}

/// An eligible current-root selection containing only policy facts.
#[derive(Clone, Eq, PartialEq)]
pub struct EligibleLineageAdmission {
    root_binding: OpaqueLineageRootBinding,
    selector_kind: DecisionAnchorTargetKindV1,
    tool_kind: GraphCorrelationToolV1,
    evidence_purpose: Option<DecisionEvidenceKindV1>,
}

impl EligibleLineageAdmission {
    pub fn new(
        root_binding: String,
        selector_kind: DecisionAnchorTargetKindV1,
        tool_kind: GraphCorrelationToolV1,
        evidence_purpose: Option<DecisionEvidenceKindV1>,
    ) -> Option<Self> {
        let capable = matches!(
            (tool_kind, selector_kind, evidence_purpose),
            (
                GraphCorrelationToolV1::SearchCode,
                DecisionAnchorTargetKindV1::Pattern,
                None,
            ) | (
                GraphCorrelationToolV1::TracePath,
                DecisionAnchorTargetKindV1::FunctionName,
                None,
            ) | (
                GraphCorrelationToolV1::GetCodeSnippet,
                DecisionAnchorTargetKindV1::QualifiedName,
                Some(_),
            )
        );
        capable.then_some(Self {
            root_binding: OpaqueLineageRootBinding::new(root_binding)?,
            selector_kind,
            tool_kind,
            evidence_purpose,
        })
    }

    pub const fn selector_kind(&self) -> DecisionAnchorTargetKindV1 {
        self.selector_kind
    }

    pub const fn tool_kind(&self) -> GraphCorrelationToolV1 {
        self.tool_kind
    }

    pub const fn evidence_purpose(&self) -> Option<DecisionEvidenceKindV1> {
        self.evidence_purpose
    }

    /// Compares a trusted lineage root without exposing this process-local
    /// binding to callers, formatting, messages, or serialization.
    pub fn matches_root(&self, candidate: &str) -> bool {
        self.root_binding.0 == candidate
    }
}

impl fmt::Debug for EligibleLineageAdmission {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EligibleLineageAdmission")
            .field("root_binding", &self.root_binding)
            .field("selector_kind", &self.selector_kind)
            .field("tool_kind", &self.tool_kind)
            .field("evidence_purpose", &self.evidence_purpose)
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
struct OpaqueLineageRootBinding(String);

impl OpaqueLineageRootBinding {
    fn new(value: String) -> Option<Self> {
        valid_root_binding(&value).then_some(Self(value))
    }
}

impl fmt::Debug for OpaqueLineageRootBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<opaque-lineage-root>")
    }
}

fn valid_root_binding(value: &str) -> bool {
    value.len() == 36
        && value.as_bytes().get(14) == Some(&b'4')
        && matches!(value.as_bytes().get(19), Some(b'8' | b'9' | b'a' | b'b'))
        && value.bytes().enumerate().all(|(index, byte)| {
            matches!(index, 8 | 13 | 18 | 23) && byte == b'-'
                || !matches!(index, 8 | 13 | 18 | 23)
                    && byte.is_ascii_hexdigit()
                    && !byte.is_ascii_uppercase()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eligible_debug_redacts_the_process_local_root() {
        const ROOT: &str = "00000000-0000-4000-8000-000000000001";
        let admission = EligibleLineageAdmission::new(
            ROOT.to_string(),
            DecisionAnchorTargetKindV1::QualifiedName,
            GraphCorrelationToolV1::GetCodeSnippet,
            Some(DecisionEvidenceKindV1::FocusedTest),
        )
        .expect("closed eligible admission");
        assert!(admission.matches_root(ROOT));
        let debug = format!("{admission:?}");
        assert!(debug.contains("FocusedTest"));
        assert!(!debug.contains(ROOT));
    }
}
