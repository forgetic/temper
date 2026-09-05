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

use super::{
    DecisionAnchorLineages, ExpandedRecoverySelector, published_handoff::PublishedRecoveryHandoff,
    target::WorkspaceTargetRegistry,
};
use crate::codebase_memory::scope::WorkspaceScope;
use crate::mcp::McpToolResultPart;

/// One run-local wrapper registry shared by result recording and pre-provider
/// admission. The mutexes protect only bounded in-memory selector and opaque
/// target metadata.
pub(crate) struct DecisionAnchorLineageRegistry {
    pub(super) lineages: Mutex<DecisionAnchorLineages>,
    pub(super) published_handoff: Mutex<Option<PublishedRecoveryHandoff>>,
    targets: Mutex<WorkspaceTargetRegistry>,
    scope: Arc<WorkspaceScope>,
}

impl DecisionAnchorLineageRegistry {
    pub(crate) fn new(scope: Arc<WorkspaceScope>) -> Self {
        Self {
            lineages: Mutex::new(DecisionAnchorLineages::default()),
            published_handoff: Mutex::new(None),
            targets: Mutex::new(WorkspaceTargetRegistry::default()),
            scope,
        }
    }
    #[cfg(test)]
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

    pub(crate) fn record_with_expanded_recovery(
        &self,
        correlation: &GraphCorrelationV1,
        input: &Value,
        typed_parts: Option<&[McpToolResultPart]>,
        decision_evidence_kind: Option<DecisionEvidenceKindV1>,
        expanded: Option<&ExpandedRecoverySelector>,
    ) -> Option<DecisionAnchorLineageV1> {
        let lineage = self
            .lineages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .record_with_expanded_recovery(
                correlation,
                input,
                typed_parts,
                decision_evidence_kind,
                expanded,
            )?;
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
        let published =
            self.published_reference_disposition(tool_name, arguments, active_root, None);
        if published == Some(GraphRecoveryReferenceDispositionV1::Rejected) {
            return (
                LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::MalformedSelector),
                published,
            );
        }
        if let Some(raw_admission) =
            self.resolve_published_raw_admission(tool_name, arguments, active_root)
        {
            return raw_admission;
        }
        let inferred_arguments = self.published_source_arguments(tool_name, arguments, active_root);
        let arguments = inferred_arguments.as_ref().unwrap_or(arguments);
        let (outcome, disposition) = self
            .lineages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .resolve_for_active_root_with_recovery(tool_name, arguments, active_root);
        if published == Some(GraphRecoveryReferenceDispositionV1::Recognized)
            && matches!(outcome, LineageAdmissionOutcome::Eligible(_))
            && !self.select_published_reference(tool_name, arguments, active_root)
        {
            return (
                LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::MalformedSelector),
                Some(GraphRecoveryReferenceDispositionV1::Rejected),
            );
        }
        let disposition = published
            .map(|_| {
                if matches!(outcome, LineageAdmissionOutcome::Eligible(_)) {
                    GraphRecoveryReferenceDispositionV1::Recognized
                } else {
                    GraphRecoveryReferenceDispositionV1::Rejected
                }
            })
            .or(disposition);
        (outcome, disposition)
    }

    fn recovery_reference_disposition(
        &self,
        tool_name: &str,
        arguments: &Value,
        active_root: Option<&str>,
    ) -> Option<GraphRecoveryReferenceDispositionV1> {
        self.published_reference_disposition(tool_name, arguments, active_root, None)
    }

    fn active_root_recovery_selectors(
        &self,
        active_root: &str,
        action: temper_protocol_activity::GraphRecoveryActionV1,
    ) -> Vec<OpaqueRecoverySelectorReference> {
        let references = self
            .lineages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .active_root_recovery_selectors(active_root, action)
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        if references.is_empty() {
            return Vec::new();
        }
        let mut published_handoff = self
            .published_handoff
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let selected_reference = published_handoff
            .as_ref()
            .filter(|handoff| {
                handoff.root_binding == active_root
                    && handoff.action == action
                    && handoff.selected_reference.as_ref().is_some_and(|selected| {
                        references.iter().any(|reference| reference == selected)
                    })
            })
            .and_then(|handoff| handoff.selected_reference.clone());
        *published_handoff = Some(PublishedRecoveryHandoff {
            root_binding: active_root.to_string(),
            action,
            references: references.clone(),
            selected_reference,
        });
        drop(published_handoff);
        references
            .into_iter()
            .filter_map(OpaqueRecoverySelectorReference::new)
            .collect()
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
    pub(super) fn resolve_for_active_root_with_recovery(
        &mut self,
        tool_name: &str,
        arguments: &Value,
        active_root: Option<&str>,
    ) -> (
        LineageAdmissionOutcome,
        Option<GraphRecoveryReferenceDispositionV1>,
    ) {
        let presented_reference =
            self.recovery_reference_disposition(tool_name, arguments, active_root);
        if GraphCorrelationToolV1::from_public_name(tool_name)
            == Some(GraphCorrelationToolV1::TracePath)
        {
            match self.reserve_implementation_trace_reference(arguments, active_root) {
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
                        self.resolve_for_active_root(tool_name, arguments, active_root),
                        presented_reference.or_else(|| {
                            active_root
                                .is_some()
                                .then_some(GraphRecoveryReferenceDispositionV1::Missing)
                        }),
                    );
                }
            }
        }
        let mut outcome = self.resolve_for_active_root(tool_name, arguments, active_root);
        if GraphCorrelationToolV1::from_public_name(tool_name)
            == Some(GraphCorrelationToolV1::GetCodeSnippet)
            && active_root.is_some()
            && outcome
                == LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::UnknownSelector)
        {
            let sibling = self.resolve_for_active_root(tool_name, arguments, None);
            if matches!(
                &sibling,
                LineageAdmissionOutcome::Eligible(admission)
                    if admission.evidence_purpose()
                        == Some(DecisionEvidenceKindV1::FocusedTest)
            ) {
                outcome = sibling;
            }
        }
        let disposition = presented_reference.map(|_| {
            if matches!(&outcome, LineageAdmissionOutcome::Eligible(_)) {
                GraphRecoveryReferenceDispositionV1::Recognized
            } else {
                GraphRecoveryReferenceDispositionV1::Rejected
            }
        });
        (outcome, disposition)
    }

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
        let recovery_root = self
            .recovery_selector(tool_name, input, evidence_purpose)
            .ok()
            .flatten()
            .map(|(_, reference)| reference.root_binding.clone());
        if self
            .validate_recovery_selector(tool_name, input, evidence_purpose)
            .is_err()
        {
            return Ineligible(MalformedSelector);
        }
        let Some(selector) = self.selector_for_input(target_kind, input) else {
            return Ineligible(MalformedSelector);
        };
        let binding = match recovery_root.as_ref() {
            Some(root) => {
                if active_root.is_some_and(|active_root| active_root != root) {
                    return Ineligible(UnknownSelector);
                }
                match self.root_selectors.get(&(root.clone(), selector.clone())) {
                    Some(binding) => binding.clone(),
                    None => return Ineligible(UnknownSelector),
                }
            }
            None => match self.selectors.get(&selector) {
                Some(Some(binding)) => binding.clone(),
                Some(None) => return Ineligible(AmbiguousSelector),
                None => return Ineligible(UnknownSelector),
            },
        };
        if active_root.is_some_and(|active_root| binding.root_binding != active_root) {
            return Ineligible(UnknownSelector);
        }
        let used_recovery_reference = object[expected_field]
            .as_str()
            .is_some_and(|value| value.starts_with(super::RECOVERY_SELECTOR_REFERENCE_PREFIX));
        if binding.recovery_reference_required && !used_recovery_reference {
            return Ineligible(IncapableSelection);
        }
        let binding_root = binding.root_binding.clone();
        let binding_target_digests = binding.canonical_target_digests.clone();
        if tool_kind == GraphCorrelationToolV1::TracePath && recovery_purpose.is_none() {
            let status = match binding
                .implementation_evidence_result
                .then_some(binding.implementation_traversal_readiness)
            {
                Some(super::ImplementationTraversalReadiness::Partial) => {
                    self.transition_equivalent_readiness(
                        &binding_root,
                        &binding_target_digests,
                        super::ImplementationTraversalReadiness::Partial,
                        super::ImplementationTraversalReadiness::RecheckAvailable,
                    );
                    Some(TraversalNotReady)
                }
                Some(super::ImplementationTraversalReadiness::RecheckAvailable) => {
                    self.transition_equivalent_readiness(
                        &binding_root,
                        &binding_target_digests,
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
        let readiness_recheck = tool_kind == GraphCorrelationToolV1::GetCodeSnippet
            && evidence_purpose == Some(DecisionEvidenceKindV1::Implementation)
            && self.is_implementation_trace_reference(input);
        if readiness_recheck {
            if binding.implementation_traversal_readiness
                != super::ImplementationTraversalReadiness::RecheckAvailable
            {
                return Ineligible(IncapableSelection);
            }
            self.transition_equivalent_readiness(
                &binding_root,
                &binding_target_digests,
                super::ImplementationTraversalReadiness::RecheckAvailable,
                super::ImplementationTraversalReadiness::RecheckPending,
            );
            return EligibleLineageAdmission::implementation_traversal_readiness_recheck(
                binding_root,
                selector.kind,
            )
            .map(Eligible)
            .unwrap_or(Ineligible(IncapableSelection));
        }
        if evidence_purpose == Some(DecisionEvidenceKindV1::Implementation)
            && !binding.implementation_evidence_result
            && self.root_selectors.values().any(|candidate| {
                candidate.root_binding == binding_root && candidate.implementation_evidence_result
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
        root_binding: &str,
        target_digests: &std::collections::BTreeSet<String>,
        from: super::ImplementationTraversalReadiness,
        to: super::ImplementationTraversalReadiness,
    ) {
        for binding in self.selectors.values_mut().flatten() {
            if binding.root_binding == root_binding
                && !binding.canonical_target_digests.is_disjoint(target_digests)
                && binding.implementation_traversal_readiness == from
            {
                binding.implementation_traversal_readiness = to;
            }
        }
        for binding in self.root_selectors.values_mut() {
            if binding.root_binding == root_binding
                && !binding.canonical_target_digests.is_disjoint(target_digests)
                && binding.implementation_traversal_readiness == from
            {
                binding.implementation_traversal_readiness = to;
            }
        }
    }
}
