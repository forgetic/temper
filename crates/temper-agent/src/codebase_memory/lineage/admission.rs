//! Closed pre-provider resolution over the wrapper-local lineage registry.

use std::sync::{Arc, Mutex};

use serde_json::Value;
use temper_agent_core::{
    EligibleLineageAdmission, InvocationTargetAdmission, LineageAdmissionOutcome,
    LineageAdmissionResolver, LineageAdmissionStatus, OpaqueRecoverySelectorReference,
    TargetAdmissionOutcome,
};
use temper_protocol_activity::{
    DecisionAnchorLineageV1, DecisionEvidenceKindV1, GraphCorrelationTargetKindV1,
    GraphCorrelationToolV1, GraphCorrelationV1, GraphRecoveryReferenceDispositionV1,
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

    pub(crate) fn recovery_selector_guidance(
        &self,
        lineage: &DecisionAnchorLineageV1,
    ) -> Option<String> {
        self.lineages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .recovery_selector_guidance(&lineage.root_binding)
    }

    pub(crate) fn expand_recovery_selector(
        &self,
        tool_name: &str,
        input: &mut Value,
        evidence_kind: Option<DecisionEvidenceKindV1>,
    ) -> Result<bool, ()> {
        self.lineages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .expand_recovery_selector(tool_name, input, evidence_kind)
    }
}

impl LineageAdmissionResolver for DecisionAnchorLineageRegistry {
    fn resolve(&self, tool_name: &str, arguments: &Value) -> LineageAdmissionOutcome {
        let mut lineages = self
            .lineages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        lineages.resolve_for_active_root(tool_name, arguments, None)
    }

    fn resolve_for_active_root(
        &self,
        tool_name: &str,
        arguments: &Value,
        active_root: Option<&str>,
    ) -> LineageAdmissionOutcome {
        let mut lineages = self
            .lineages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        lineages.resolve_for_active_root(tool_name, arguments, active_root)
    }

    fn resolve_for_active_root_with_recovery(
        &self,
        tool_name: &str,
        arguments: &Value,
        active_root: Option<&str>,
    ) -> (
        LineageAdmissionOutcome,
        Option<GraphRecoveryReferenceDispositionV1>,
    ) {
        let mut lineages = self
            .lineages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if GraphCorrelationToolV1::from_public_name(tool_name)
            == Some(GraphCorrelationToolV1::TracePath)
        {
            match lineages.reserve_implementation_trace_reference(arguments, active_root) {
                Ok(Some(admission)) => {
                    return (
                        LineageAdmissionOutcome::Eligible(admission),
                        Some(GraphRecoveryReferenceDispositionV1::Recognized),
                    );
                }
                Err(()) => {
                    return (
                        LineageAdmissionOutcome::Ineligible(
                            LineageAdmissionStatus::MalformedSelector,
                        ),
                        Some(GraphRecoveryReferenceDispositionV1::Rejected),
                    );
                }
                Ok(None) => {
                    return (
                        lineages.resolve_for_active_root(tool_name, arguments, active_root),
                        active_root
                            .is_some()
                            .then_some(GraphRecoveryReferenceDispositionV1::Missing),
                    );
                }
            }
        }
        let outcome = lineages.resolve_for_active_root(tool_name, arguments, active_root);
        if GraphCorrelationToolV1::from_public_name(tool_name)
            == Some(GraphCorrelationToolV1::GetCodeSnippet)
            && active_root.is_some()
            && outcome
                == LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::UnknownSelector)
        {
            let sibling = lineages.resolve_for_active_root(tool_name, arguments, None);
            if matches!(
                &sibling,
                LineageAdmissionOutcome::Eligible(admission)
                    if admission.evidence_purpose()
                        == Some(DecisionEvidenceKindV1::FocusedTest)
            ) {
                return (sibling, None);
            }
        }
        (outcome, None)
    }

    fn trace_recovery_selector(
        &self,
        active_root: &str,
    ) -> Option<OpaqueRecoverySelectorReference> {
        let lineages = self
            .lineages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        OpaqueRecoverySelectorReference::new(
            lineages
                .implementation_trace_recovery_selector(active_root)?
                .to_string(),
        )
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
    #[cfg(test)]
    pub(crate) fn resolve(&mut self, tool_name: &str, input: &Value) -> LineageAdmissionOutcome {
        self.resolve_for_active_root(tool_name, input, None)
    }

    pub(crate) fn resolve_for_active_root(
        &mut self,
        tool_name: &str,
        input: &Value,
        active_root: Option<&str>,
    ) -> LineageAdmissionOutcome {
        use LineageAdmissionOutcome::{Eligible, Ineligible};
        use LineageAdmissionStatus::{
            AmbiguousSelector, BroadSelector, IncapableSelection, MalformedSelector,
            TraversalNotReady, TraversalReadinessExhausted, UnknownSelector, UnsupportedTool,
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
                    if !matches!(present[0], "name_pattern" | "qn_pattern") {
                        return Ineligible(IncapableSelection);
                    }
                    if object.get("label").is_none() {
                        return Ineligible(BroadSelector);
                    }
                    if object.get("label").and_then(Value::as_str).is_none() {
                        return Ineligible(MalformedSelector);
                    }
                    return self
                        .resolve_exact_graph_narrowing(input, active_root)
                        .map(Eligible)
                        .unwrap_or_else(Ineligible);
                }
                return Ineligible(BroadSelector);
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
        if self
            .validate_recovery_selector(tool_name, input, evidence_purpose)
            .is_err()
        {
            return Ineligible(MalformedSelector);
        }
        let Some(selector) = self.selector_for_input(target_kind, input) else {
            return Ineligible(MalformedSelector);
        };
        if tool_kind == GraphCorrelationToolV1::TracePath && recovery_purpose.is_none() {
            let readiness = self
                .selectors
                .get(&selector)
                .and_then(Option::as_ref)
                .filter(|binding| binding.implementation_evidence_result)
                .map(|binding| binding.implementation_traversal_readiness);
            let status = match readiness {
                Some(super::ImplementationTraversalReadiness::Partial) => {
                    self.transition_equivalent_readiness(
                        &selector,
                        super::ImplementationTraversalReadiness::Partial,
                        super::ImplementationTraversalReadiness::RecheckAvailable,
                    );
                    Some(TraversalNotReady)
                }
                Some(super::ImplementationTraversalReadiness::RecheckAvailable) => {
                    self.transition_equivalent_readiness(
                        &selector,
                        super::ImplementationTraversalReadiness::RecheckAvailable,
                        super::ImplementationTraversalReadiness::RecheckExhausted,
                    );
                    Some(TraversalReadinessExhausted)
                }
                Some(
                    super::ImplementationTraversalReadiness::RecheckPending
                    | super::ImplementationTraversalReadiness::RecheckExhausted,
                ) => Some(TraversalReadinessExhausted),
                Some(super::ImplementationTraversalReadiness::Ready) | None => None,
            };
            if let Some(status) = status {
                return Ineligible(status);
            }
        }
        let binding = match self.selectors.get(&selector) {
            Some(Some(binding)) => binding,
            Some(None) => return Ineligible(AmbiguousSelector),
            None => return Ineligible(UnknownSelector),
        };
        if active_root.is_some_and(|active_root| binding.root_binding != active_root) {
            return Ineligible(UnknownSelector);
        }
        let readiness_recheck = tool_kind == GraphCorrelationToolV1::GetCodeSnippet
            && evidence_purpose == Some(DecisionEvidenceKindV1::Implementation)
            && object
                .get("qualified_name")
                .and_then(Value::as_str)
                .is_some_and(|value| value.starts_with(super::RECOVERY_SELECTOR_REFERENCE_PREFIX));
        if readiness_recheck {
            if binding.implementation_traversal_readiness
                != super::ImplementationTraversalReadiness::RecheckAvailable
            {
                return Ineligible(IncapableSelection);
            }
            let root_binding = binding.root_binding.clone();
            self.transition_equivalent_readiness(
                &selector,
                super::ImplementationTraversalReadiness::RecheckAvailable,
                super::ImplementationTraversalReadiness::RecheckPending,
            );
            return EligibleLineageAdmission::implementation_traversal_readiness_recheck(
                root_binding,
                selector.kind,
            )
            .map(Eligible)
            .unwrap_or(Ineligible(IncapableSelection));
        }
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

    fn transition_equivalent_readiness(
        &mut self,
        selector: &super::Selector,
        from: super::ImplementationTraversalReadiness,
        to: super::ImplementationTraversalReadiness,
    ) {
        let Some(Some(binding)) = self.selectors.get(selector) else {
            return;
        };
        let root_binding = binding.root_binding.clone();
        let target_digests = binding.canonical_target_digests.clone();
        for binding in self.selectors.values_mut().flatten() {
            if binding.root_binding == root_binding
                && !binding
                    .canonical_target_digests
                    .is_disjoint(&target_digests)
                && binding.implementation_traversal_readiness == from
            {
                binding.implementation_traversal_readiness = to;
            }
        }
    }
}
