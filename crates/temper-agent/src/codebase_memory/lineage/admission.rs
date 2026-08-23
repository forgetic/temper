//! Closed pre-provider resolution over the wrapper-local lineage registry.

use std::sync::Mutex;

use serde_json::Value;
use temper_agent_core::{
    EligibleLineageAdmission, LineageAdmissionOutcome, LineageAdmissionResolver,
    LineageAdmissionStatus,
};
use temper_protocol_activity::{
    DecisionAnchorLineageV1, DecisionEvidenceKindV1, GraphCorrelationTargetKindV1,
    GraphCorrelationToolV1, GraphCorrelationV1,
};

use super::DecisionAnchorLineages;
use crate::mcp::McpToolResultPart;

/// One run-local wrapper registry shared by result recording and pre-provider
/// admission. The mutex protects only bounded in-memory selector metadata.
#[derive(Default)]
pub(crate) struct DecisionAnchorLineageRegistry {
    lineages: Mutex<DecisionAnchorLineages>,
}

impl DecisionAnchorLineageRegistry {
    pub(crate) fn record_with_evidence_kind(
        &self,
        correlation: &GraphCorrelationV1,
        input: &Value,
        typed_parts: Option<&[McpToolResultPart]>,
        decision_evidence_kind: Option<DecisionEvidenceKindV1>,
    ) -> Option<DecisionAnchorLineageV1> {
        self.lineages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .record_with_evidence_kind(correlation, input, typed_parts, decision_evidence_kind)
    }
}

impl LineageAdmissionResolver for DecisionAnchorLineageRegistry {
    fn resolve(&self, tool_name: &str, arguments: &Value) -> LineageAdmissionOutcome {
        self.lineages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .resolve(tool_name, arguments)
    }
}

impl DecisionAnchorLineages {
    pub(crate) fn resolve(&self, tool_name: &str, input: &Value) -> LineageAdmissionOutcome {
        use LineageAdmissionOutcome::{Eligible, Ineligible};
        use LineageAdmissionStatus::{
            AmbiguousSelector, BroadSelector, IncapableSelection, MalformedSelector,
            UnknownSelector, UnsupportedTool,
        };

        let Some(tool_kind) = GraphCorrelationToolV1::from_public_name(tool_name) else {
            return Ineligible(UnsupportedTool);
        };
        let Some(object) = input.as_object() else {
            return Ineligible(MalformedSelector);
        };
        let selector_fields = [
            "query",
            "name_pattern",
            "qn_pattern",
            "pattern",
            "function_name",
            "qualified_name",
        ];
        let present = selector_fields
            .iter()
            .filter(|field| object.contains_key(**field))
            .copied()
            .collect::<Vec<_>>();
        if present.len() != 1 || object[present[0]].as_str().is_none() {
            return Ineligible(MalformedSelector);
        }
        let (expected_field, target_kind) = match tool_kind {
            GraphCorrelationToolV1::SearchGraph => {
                return Ineligible(
                    if matches!(present[0], "query" | "name_pattern" | "qn_pattern") {
                        BroadSelector
                    } else {
                        IncapableSelection
                    },
                );
            }
            GraphCorrelationToolV1::SearchCode => {
                ("pattern", GraphCorrelationTargetKindV1::Pattern)
            }
            GraphCorrelationToolV1::TracePath => {
                ("function_name", GraphCorrelationTargetKindV1::FunctionName)
            }
            GraphCorrelationToolV1::GetCodeSnippet => (
                "qualified_name",
                GraphCorrelationTargetKindV1::QualifiedName,
            ),
        };
        if present[0] != expected_field {
            return Ineligible(IncapableSelection);
        }
        let evidence_purpose = match object.get("decision_evidence_kind") {
            Some(value) if tool_kind == GraphCorrelationToolV1::GetCodeSnippet => {
                match serde_json::from_value::<DecisionEvidenceKindV1>(value.clone()) {
                    Ok(kind) => Some(kind),
                    Err(_) => return Ineligible(MalformedSelector),
                }
            }
            Some(_) => return Ineligible(IncapableSelection),
            None if tool_kind == GraphCorrelationToolV1::GetCodeSnippet => {
                return Ineligible(IncapableSelection);
            }
            None => None,
        };
        let Some(selector) = self.selector_for_input(target_kind, input) else {
            return Ineligible(MalformedSelector);
        };
        let binding = match self.selectors.get(&selector) {
            Some(Some(binding)) => binding,
            Some(None) => return Ineligible(AmbiguousSelector),
            None => return Ineligible(UnknownSelector),
        };
        EligibleLineageAdmission::new(
            binding.root_binding.clone(),
            selector.kind,
            tool_kind,
            evidence_purpose,
        )
        .map(Eligible)
        .unwrap_or(Ineligible(IncapableSelection))
    }
}
