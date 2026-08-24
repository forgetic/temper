//! Closed pre-provider resolution over the wrapper-local lineage registry.

use std::sync::{Arc, Mutex};

use serde_json::Value;
use temper_agent_core::{
    EligibleLineageAdmission, InvocationTargetAdmission, LineageAdmissionOutcome,
    LineageAdmissionResolver, LineageAdmissionStatus, TargetAdmissionOutcome,
};
use temper_protocol_activity::{
    DecisionAnchorLineageV1, DecisionEvidenceKindV1, GraphCorrelationTargetKindV1,
    GraphCorrelationToolV1, GraphCorrelationV1,
};

use super::{DecisionAnchorLineages, target::WorkspaceTargetRegistry};
use crate::codebase_memory::scope::WorkspaceScope;
use crate::mcp::McpToolResultPart;

/// One run-local wrapper registry shared by result recording and pre-provider
/// admission. The mutexes protect only bounded in-memory selector and opaque
/// target metadata.
pub(crate) struct DecisionAnchorLineageRegistry {
    lineages: Mutex<DecisionAnchorLineages>,
    targets: Mutex<WorkspaceTargetRegistry>,
    scope: Arc<WorkspaceScope>,
}

impl DecisionAnchorLineageRegistry {
    pub(crate) fn new(scope: Arc<WorkspaceScope>) -> Self {
        Self {
            lineages: Mutex::new(DecisionAnchorLineages::default()),
            targets: Mutex::new(WorkspaceTargetRegistry::default()),
            scope,
        }
    }
    pub(crate) fn record_with_evidence_kind(
        &self,
        correlation: &GraphCorrelationV1,
        input: &Value,
        typed_parts: Option<&[McpToolResultPart]>,
        decision_evidence_kind: Option<DecisionEvidenceKindV1>,
    ) -> Option<DecisionAnchorLineageV1> {
        let lineage = self
            .lineages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .record_with_evidence_kind(correlation, input, typed_parts, decision_evidence_kind)?;
        self.targets
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .record_source(&self.scope, &lineage, input, typed_parts);
        Some(lineage)
    }
}

impl LineageAdmissionResolver for DecisionAnchorLineageRegistry {
    fn resolve(&self, tool_name: &str, arguments: &Value) -> LineageAdmissionOutcome {
        let mut lineages = self
            .lineages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        lineages.resolve(tool_name, arguments)
    }

    fn resolve_source_target(&self, lineage: &DecisionAnchorLineageV1) -> TargetAdmissionOutcome {
        self.targets
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .resolve_source(lineage)
    }

    fn resolve_invocation_targets(
        &self,
        tool_name: &str,
        arguments: &Value,
    ) -> InvocationTargetAdmission {
        self.targets
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .resolve_invocation(&self.scope, tool_name, arguments)
    }
}

impl DecisionAnchorLineages {
    pub(crate) fn resolve(&mut self, tool_name: &str, input: &Value) -> LineageAdmissionOutcome {
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
                if present[0] != "query" {
                    return Ineligible(if matches!(present[0], "name_pattern" | "qn_pattern") {
                        BroadSelector
                    } else {
                        IncapableSelection
                    });
                }
                let Some(query) = object.get("query").and_then(Value::as_str) else {
                    return Ineligible(MalformedSelector);
                };
                let Some(query_digest) = GraphCorrelationV1::target_digest(query) else {
                    return Ineligible(MalformedSelector);
                };
                let mut roots = self
                    .focused_test_recovery
                    .iter()
                    .filter_map(|(root, state)| {
                        (*state == super::FocusedTestRecoveryState::TraversalReturnedEmpty)
                            .then_some(root.clone())
                    });
                let Some(root_binding) = roots.next() else {
                    return Ineligible(BroadSelector);
                };
                if roots.next().is_some() {
                    return Ineligible(AmbiguousSelector);
                }
                match self.semantic_fallback_queries.get(&query_digest) {
                    Some(Some(existing)) if existing != &root_binding => {
                        self.semantic_fallback_queries.insert(query_digest, None);
                        return Ineligible(AmbiguousSelector);
                    }
                    Some(None) => return Ineligible(AmbiguousSelector),
                    Some(Some(_)) => {}
                    None => {
                        self.semantic_fallback_queries
                            .insert(query_digest, Some(root_binding.clone()));
                    }
                }
                self.focused_test_recovery.insert(
                    root_binding.clone(),
                    super::FocusedTestRecoveryState::FallbackPending,
                );
                return EligibleLineageAdmission::focused_test_semantic_fallback(
                    root_binding,
                    temper_protocol_activity::DecisionAnchorTargetKindV1::GraphQuery,
                )
                .map(Eligible)
                .unwrap_or(Ineligible(IncapableSelection));
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
        let recovery_purpose = if tool_kind == GraphCorrelationToolV1::TracePath {
            match object.get("include_tests") {
                Some(value) if value.as_bool() == Some(true) => {
                    if object.get("mode").and_then(Value::as_str) != Some("calls")
                        || object.get("direction").and_then(Value::as_str) != Some("inbound")
                    {
                        return Ineligible(IncapableSelection);
                    }
                    Some(DecisionEvidenceKindV1::FocusedTest)
                }
                Some(value) if value.as_bool() == Some(false) => None,
                Some(_) => return Ineligible(MalformedSelector),
                None => None,
            }
        } else {
            None
        };
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
        if evidence_purpose == Some(DecisionEvidenceKindV1::Implementation)
            && !binding.implementation_evidence_result
            && self.selectors.values().flatten().any(|candidate| {
                candidate.root_binding == binding.root_binding
                    && candidate.implementation_evidence_result
            })
        {
            return Ineligible(IncapableSelection);
        }
        if evidence_purpose == Some(DecisionEvidenceKindV1::Caller)
            && !binding.caller_traversal_result
        {
            return Ineligible(IncapableSelection);
        }
        if recovery_purpose == Some(DecisionEvidenceKindV1::FocusedTest)
            && !binding.caller_evidence_result
        {
            return Ineligible(IncapableSelection);
        }
        if evidence_purpose == Some(DecisionEvidenceKindV1::FocusedTest)
            && !binding.focused_test_result
        {
            return Ineligible(IncapableSelection);
        }
        if recovery_purpose == Some(DecisionEvidenceKindV1::FocusedTest) {
            return EligibleLineageAdmission::focused_test_traversal(
                binding.root_binding.clone(),
                selector.kind,
            )
            .map(Eligible)
            .unwrap_or(Ineligible(IncapableSelection));
        }
        if tool_kind == GraphCorrelationToolV1::TracePath {
            if object
                .get("direction")
                .and_then(Value::as_str)
                .is_some_and(|direction| direction != "inbound")
                || object
                    .get("mode")
                    .and_then(Value::as_str)
                    .is_some_and(|mode| mode != "calls")
            {
                return Ineligible(IncapableSelection);
            }
            return EligibleLineageAdmission::implementation_caller_traversal(
                binding.root_binding.clone(),
                selector.kind,
            )
            .map(Eligible)
            .unwrap_or(Ineligible(IncapableSelection));
        }
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
