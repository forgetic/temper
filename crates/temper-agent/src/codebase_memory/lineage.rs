//! Process-local provider-result matching for typed decision-anchor lineage.
//!
//! Bounded, typed MCP result parts stay here; policy receives only the opaque root and canonical
//! target-kind aggregate in `DecisionAnchorLineageV1`.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use temper_protocol_activity::{
    CallerDiscoveryOutcomeV1, DecisionAnchorLineageStageV1, DecisionAnchorLineageV1,
    DecisionAnchorTargetKindV1, DecisionEvidenceKindV1, GraphCorrelationTargetKindV1,
    GraphCorrelationToolV1, GraphCorrelationV1, GraphRecoveryActionV1,
};
use uuid::Uuid;

use crate::mcp::McpToolResultPart;

const MAX_RESULT_TARGETS: usize = 64;

mod active_root_handoff;
mod admission;
mod candidate_projection;
mod exact_narrowing;
mod focused_test;
mod published_handoff;
mod recovery_record;
mod recovery_selector;
mod selection;
mod target;

pub(super) use admission::DecisionAnchorLineageRegistry;
use candidate_projection::{CandidateCollection, provider_candidates};
use exact_narrowing::{ExactGraphSelector, PendingExactGraphNarrowing};
use focused_test::{FocusedTestDiscovery, SelectorOrigin, focused_test_discovery};
use recovery_record::ExpandedRecoverySelector;
use recovery_selector::{
    CandidateRecovery, RECOVERY_SELECTOR_REFERENCE_PREFIX, RecoverySelectorKey,
    RecoverySelectorPurpose, RecoverySelectorReference, RecoverySelectorState,
};
use selection::{
    ImplementationTraversalEvidence, canonical_function_name, canonical_qualified_name,
    canonical_target_digests, implementation_root_candidates, implementation_traversal_evidence,
    provider_caller_candidates, provider_explicit_source_names, provider_function_name_for_source,
    terminal_function_name,
};

#[derive(Clone, Default)]
pub(super) struct DecisionAnchorLineages {
    /// `None` marks a value offered by more than one root; it cannot advance either root.
    selectors: BTreeMap<Selector, Option<SelectorBinding>>,
    /// Root-qualified bindings preserve opaque-reference authority when the
    /// same provider selector is independently returned by parallel roots.
    root_selectors: BTreeMap<(String, Selector), SelectorBinding>,
    recovery_references: BTreeMap<RecoverySelectorKey, Vec<String>>,
    recovery_reference_selectors: BTreeMap<String, RecoverySelectorReference>,
    exact_graph_selectors: BTreeMap<ExactGraphSelector, BTreeMap<String, Option<BTreeSet<String>>>>,
    pending_exact_graph_narrowings:
        BTreeMap<ExactGraphSelector, Option<PendingExactGraphNarrowing>>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Selector {
    kind: DecisionAnchorTargetKindV1,
    value: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Candidate {
    kind: DecisionAnchorTargetKindV1,
    provider_kind: DecisionAnchorTargetKindV1,
    value: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SelectorBinding {
    root_binding: String,
    canonical_target_digests: BTreeSet<String>,
    implementation_evidence_result: bool,
    caller_traversal_result: bool,
    caller_evidence_result: bool,
    focused_test_result: bool,
    focused_test_confirmation_required: bool,
    recovery_reference_required: bool,
    implementation_traversal_readiness: ImplementationTraversalReadiness,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ImplementationTraversalReadiness {
    Ready,
    Partial,
    RecheckAvailable,
    RecheckPending,
    RecheckExhausted,
}

impl DecisionAnchorLineages {
    /// Derives one trusted output record after a successful, complete targeted
    /// wrapper result. Callers must not invoke this for provider errors, empty
    /// output, or truncation.
    #[cfg(test)]
    pub(super) fn record(
        &mut self,
        correlation: &GraphCorrelationV1,
        input: &Value,
        typed_parts: Option<&[McpToolResultPart]>,
    ) -> Option<DecisionAnchorLineageV1> {
        self.record_with_evidence_kind(correlation, input, typed_parts, None)
    }

    /// Records lineage with an optional wrapper-validated source purpose.
    #[cfg(test)]
    pub(super) fn record_with_evidence_kind(
        &mut self,
        correlation: &GraphCorrelationV1,
        input: &Value,
        typed_parts: Option<&[McpToolResultPart]>,
        decision_evidence_kind: Option<DecisionEvidenceKindV1>,
    ) -> Option<DecisionAnchorLineageV1> {
        self.record_with_recovery_root(
            correlation,
            input,
            typed_parts,
            decision_evidence_kind,
            None,
        )
    }

    fn record_with_recovery_root(
        &mut self,
        correlation: &GraphCorrelationV1,
        input: &Value,
        typed_parts: Option<&[McpToolResultPart]>,
        decision_evidence_kind: Option<DecisionEvidenceKindV1>,
        recovery_root: Option<&str>,
    ) -> Option<DecisionAnchorLineageV1> {
        if !correlation.is_valid() {
            return None;
        }
        let target_kind =
            DecisionAnchorTargetKindV1::from_graph_correlation(correlation.target_kind);
        let exact_graph_identities = (correlation.tool == GraphCorrelationToolV1::SearchGraph)
            .then(|| exact_narrowing::provider_exact_graph_identities(typed_parts))
            .flatten();
        let exact_narrowing =
            self.consume_exact_graph_narrowing(correlation, input, exact_graph_identities.as_ref());
        let input_selector = self.selector_for_input(correlation.target_kind, input);
        let selector_binding = exact_narrowing
            .as_ref()
            .map(|narrowing| {
                SelectorBinding::new(
                    narrowing.root_binding.clone(),
                    narrowing.canonical_target_digests.clone(),
                )
            })
            .or_else(|| {
                input_selector
                    .as_ref()
                    .and_then(|selector| match recovery_root {
                        Some(root) => self
                            .root_selectors
                            .get(&(root.to_string(), selector.clone()))
                            .cloned(),
                        None => self.selectors.get(selector).cloned().flatten(),
                    })
            });
        let admitted_evidence_kind = self.admitted_evidence_kind(
            decision_evidence_kind,
            input_selector.as_ref(),
            selector_binding.as_ref(),
            typed_parts,
        );
        let matched_root = selector_binding
            .as_ref()
            .map(|binding| binding.root_binding.clone());
        let (root_binding, stage, canonical_target_digests) = match matched_root {
            Some(root_binding) => (
                root_binding,
                DecisionAnchorLineageStageV1::CarryForward,
                selector_binding
                    .as_ref()
                    .map(|binding| binding.canonical_target_digests.clone())
                    .unwrap_or_default(),
            ),
            None => (
                Uuid::new_v4().to_string(),
                DecisionAnchorLineageStageV1::Root,
                BTreeSet::new(),
            ),
        };

        let is_caller_traversal = correlation.tool == GraphCorrelationToolV1::TracePath
            && input
                .get("direction")
                .and_then(Value::as_str)
                .is_none_or(|direction| direction == "inbound")
            && input
                .get("mode")
                .and_then(Value::as_str)
                .is_none_or(|mode| mode == "calls")
            && input
                .get("include_tests")
                .and_then(Value::as_bool)
                .is_none_or(|include| !include)
            && selector_binding.is_some();
        let caller_candidates = is_caller_traversal
            .then(|| provider_caller_candidates(typed_parts))
            .flatten();
        let mut caller_discovery = caller_candidates.as_ref().map(|candidates| {
            if candidates.is_empty() {
                CallerDiscoveryOutcomeV1::NoEligibleSelector
            } else {
                CallerDiscoveryOutcomeV1::EligibleSelectorReturned
            }
        });

        let FocusedTestDiscovery {
            candidates: focused_tests,
            outcome: mut focused_test_discovery,
        } = focused_test_discovery(
            correlation,
            input,
            selector_binding
                .as_ref()
                .is_some_and(|binding| binding.caller_evidence_result),
            typed_parts,
        );

        let mut marked_focused_tests = None;
        let result_target_kinds = match provider_candidates(typed_parts) {
            Some(provider_candidates) => {
                let mut candidates = provider_candidates.candidates;
                if let Some(focused_tests) = focused_tests.as_ref() {
                    candidates.extend(focused_tests.iter().cloned());
                }
                let kinds = candidates.iter().map(|candidate| candidate.kind).collect();
                if let Some(narrowing) = exact_narrowing.as_ref() {
                    self.promote_exact_graph_identity(
                        &narrowing.root_binding,
                        &narrowing.canonical_target_digests,
                        &candidates,
                    );
                }
                self.register(
                    &root_binding,
                    candidates.clone(),
                    provider_candidates.projected,
                )?;
                if stage == DecisionAnchorLineageStageV1::Root
                    && matches!(
                        correlation.tool,
                        GraphCorrelationToolV1::SearchGraph | GraphCorrelationToolV1::SearchCode
                    )
                {
                    self.replace_recovery_references(
                        &root_binding,
                        RecoverySelectorPurpose::ImplementationCandidate,
                        &implementation_root_candidates(
                            &provider_candidates.provider_order,
                            focused_tests.as_ref(),
                        ),
                        true,
                    );
                }
                if !provider_candidates.projected {
                    if let Some(identities) = exact_graph_identities.as_ref() {
                        self.register_exact_graph_identities(&root_binding, identities);
                    }
                }
                if admitted_evidence_kind == Some(DecisionEvidenceKindV1::Implementation) {
                    let traversal_evidence = implementation_traversal_evidence(typed_parts);
                    let trace_provider_value =
                        provider_function_name_for_source(typed_parts, input);
                    self.mark_input_selector(
                        correlation.target_kind,
                        input,
                        &root_binding,
                        &candidates,
                        SelectorOrigin::ImplementationEvidenceResult { traversal_evidence },
                        trace_provider_value.as_deref(),
                    )?;
                }
                if admitted_evidence_kind == Some(DecisionEvidenceKindV1::Caller) {
                    self.mark_input_selector(
                        correlation.target_kind,
                        input,
                        &root_binding,
                        &candidates,
                        SelectorOrigin::CallerEvidenceResult,
                        None,
                    )?;
                }
                if let Some(callers) = caller_candidates {
                    let marked = self.mark_candidates(
                        &root_binding,
                        callers,
                        SelectorOrigin::CallerTraversalResult,
                    )?;
                    if marked == 0 {
                        caller_discovery = Some(CallerDiscoveryOutcomeV1::NoEligibleSelector);
                    }
                }
                if let Some(focused_tests) = focused_tests.as_ref() {
                    marked_focused_tests = Some(self.mark_candidates(
                        &root_binding,
                        focused_tests.clone(),
                        SelectorOrigin::FocusedTestResult,
                    )?);
                    self.prefer_explicit_source_names(
                        &root_binding,
                        RecoverySelectorPurpose::FocusedTestSource,
                        &provider_explicit_source_names(typed_parts),
                    );
                }
                kinds
            }
            None => BTreeSet::new(),
        };
        focused_test_discovery = self.record_registered_focused_test_candidates(
            focused_tests.is_some(),
            marked_focused_tests,
            focused_test_discovery,
        );
        DecisionAnchorLineageV1::new_with_route_metadata(
            root_binding,
            stage,
            target_kind,
            result_target_kinds,
            canonical_target_digests,
            admitted_evidence_kind,
            caller_discovery,
            focused_test_discovery,
        )
    }
}

/// Only valid MCP text blocks can provide JSON lineage candidates.
fn content_part_json(block: &Value) -> Option<Option<Value>> {
    let block = block.as_object()?;
    match block.get("type")?.as_str()? {
        "text" => block
            .get("text")?
            .as_str()
            .map(|text| serde_json::from_str(text).ok()),
        _ => Some(None),
    }
}

fn collect_result<C: CandidateCollection>(value: &Value, candidates: &mut C) -> Option<()> {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_result_item(value, candidates)?;
            }
        }
        Value::Object(values) => collect_result_record(values, candidates)?,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => return None,
    }
    candidates.within_result_limit().then_some(())
}

fn collect_result_item<C: CandidateCollection>(value: &Value, candidates: &mut C) -> Option<()> {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_result_item(value, candidates)?;
            }
        }
        Value::Object(values) => collect_result_record(values, candidates)?,
        Value::String(value) => insert_reference(candidates, value)?,
        Value::Null | Value::Bool(_) | Value::Number(_) => return None,
    }
    candidates.within_result_limit().then_some(())
}

fn collect_result_record<C: CandidateCollection>(
    values: &serde_json::Map<String, Value>,
    candidates: &mut C,
) -> Option<()> {
    collect_direct_symbol(values, candidates)?;

    for (field, value) in values {
        match field.as_str() {
            "results" | "semantic_results" | "semanticResults" => {
                collect_result(value, candidates)?
            }
            "callers" | "caller_list" | "callerList" | "caller_functions" | "callerFunctions"
            | "callees" | "callee_list" | "calleeList" | "callee_functions" | "calleeFunctions"
            | "symbols" | "short_symbols" | "shortSymbols" => {
                collect_reference_list_or_count(value, candidates)?
            }
            "related_source_references"
            | "relatedSourceReferences"
            | "related_source_refs"
            | "relatedSourceRefs"
            | "related_sources"
            | "relatedSources" => collect_reference_list(value, candidates)?,
            "next_target" | "nextTarget" | "function" => collect_reference(value, candidates)?,
            "source_metadata" | "sourceMetadata" => collect_source_metadata(value, candidates)?,
            "symbol" if value.is_object() => collect_reference(value, candidates)?,
            _ => {}
        }
    }
    candidates.within_result_limit().then_some(())
}

fn collect_direct_symbol<C: CandidateCollection>(
    values: &serde_json::Map<String, Value>,
    candidates: &mut C,
) -> Option<()> {
    let qualified_field = one_symbol_field(values, &["qualified_name", "qualifiedName"])?;
    let (qualified, short_from_qualified_field, invalid_qualified_field) = match &qualified_field {
        Some(value) => match canonical_qualified_name(value) {
            Some(qualified) => (Some(qualified), None, false),
            // The approved provider shape may label a short implementation
            // symbol as `qualified_name`. Preserve its useful closed
            // function representation instead of rejecting the entire
            // otherwise typed result collection.
            None => match canonical_function_name(value) {
                Some(short) => (None, Some(short), false),
                // Native graph records can prefix a qualified identity with a
                // package name that is not a Rust identifier (for example a
                // hyphenated package). Its adjacent short `name` is the
                // approved identity in that shape; without that field this
                // value remains ineligible.
                None => (None, None, true),
            },
        },
        None => (None, None, false),
    };
    let short = match one_symbol_field(
        values,
        &[
            "function_name",
            "functionName",
            "short_symbol",
            "shortSymbol",
            "short_name",
            "shortName",
            "symbol_name",
            "symbolName",
            "symbol",
        ],
    )? {
        Some(value) => Some(canonical_function_name(&value)?),
        None => None,
    };
    let selected_short = match (short_from_qualified_field, short) {
        (Some(from_qualified), Some(short)) if from_qualified == short => Some(short),
        (Some(_), Some(_)) => return None,
        (Some(short), None) | (None, Some(short)) => Some(short),
        (None, None) => None,
    };
    let display_short = if qualified_field.is_some() {
        match one_symbol_field(values, &["name"])? {
            Some(value) => Some(canonical_function_name(&value)?),
            None => None,
        }
    } else {
        None
    };
    let selected_short = match (selected_short, display_short) {
        (Some(short), Some(display)) if short == display => Some(short),
        (Some(_), Some(_)) => return None,
        (Some(short), None) | (None, Some(short)) => Some(short),
        (None, None) => None,
    };
    if invalid_qualified_field && selected_short.is_none() {
        return None;
    }

    match (qualified, selected_short) {
        (Some(qualified), Some(short)) => {
            (terminal_function_name(&qualified)? == short).then_some(())?;
            insert_qualified(candidates, qualified)?;
            insert_function(candidates, short)
        }
        (Some(qualified), None) => insert_qualified(candidates, qualified),
        (None, Some(short)) => insert_function(candidates, short),
        (None, None) => Some(()),
    }
}

fn one_symbol_field(
    values: &serde_json::Map<String, Value>,
    fields: &[&str],
) -> Option<Option<String>> {
    let mut value = None;
    for field in fields {
        let Some(candidate) = values.get(*field) else {
            continue;
        };
        // `symbol` may itself be a structured reference; its object form is
        // handled by `collect_result_record` rather than being coerced.
        if *field == "symbol" && candidate.is_object() {
            continue;
        }
        let candidate = candidate.as_str()?.to_string();
        if value.replace(candidate).is_some() {
            return None;
        }
    }
    Some(value)
}

fn collect_reference_list<C: CandidateCollection>(value: &Value, candidates: &mut C) -> Option<()> {
    for value in value.as_array()? {
        collect_reference(value, candidates)?;
    }
    Some(())
}

fn collect_reference_list_or_count<C: CandidateCollection>(
    value: &Value,
    candidates: &mut C,
) -> Option<()> {
    if value.is_u64() {
        // Source metadata reports caller cardinality under the same field name
        // used by trace results for an actual caller list.
        Some(())
    } else {
        collect_reference_list(value, candidates)
    }
}

fn collect_source_metadata<C: CandidateCollection>(
    value: &Value,
    candidates: &mut C,
) -> Option<()> {
    collect_result_record(value.as_object()?, candidates)
}

fn collect_reference<C: CandidateCollection>(value: &Value, candidates: &mut C) -> Option<()> {
    match value {
        Value::String(value) => insert_reference(candidates, value),
        Value::Object(value) => collect_result_record(value, candidates),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::Array(_) => None,
    }
}

fn insert_reference<C: CandidateCollection>(candidates: &mut C, value: &str) -> Option<()> {
    match canonical_qualified_name(value) {
        Some(value) => insert_qualified(candidates, value),
        None => insert_function(candidates, canonical_function_name(value)?),
    }
}

fn insert_qualified<C: CandidateCollection>(candidates: &mut C, value: String) -> Option<()> {
    let function = terminal_function_name(&value)?;
    insert(
        candidates,
        DecisionAnchorTargetKindV1::QualifiedName,
        DecisionAnchorTargetKindV1::Pattern,
        value.clone(),
    )?;
    // Pattern selectors commonly use the terminal symbol returned beside a
    // provider-qualified identity. Retain that closed representation too;
    // the registry's ambiguity handling prevents a shared terminal name from
    // binding across distinct roots.
    insert(
        candidates,
        DecisionAnchorTargetKindV1::QualifiedName,
        DecisionAnchorTargetKindV1::Pattern,
        function.clone(),
    )?;
    insert(
        candidates,
        DecisionAnchorTargetKindV1::QualifiedName,
        DecisionAnchorTargetKindV1::QualifiedName,
        value,
    )?;
    // A source-read wrapper accepts a short `qualified_name` selector. Keep
    // the provider record's direct name as that closed representation too.
    insert(
        candidates,
        DecisionAnchorTargetKindV1::QualifiedName,
        DecisionAnchorTargetKindV1::QualifiedName,
        function.clone(),
    )?;
    insert(
        candidates,
        DecisionAnchorTargetKindV1::QualifiedName,
        DecisionAnchorTargetKindV1::FunctionName,
        function,
    )
}

fn insert_function<C: CandidateCollection>(candidates: &mut C, value: String) -> Option<()> {
    insert(
        candidates,
        DecisionAnchorTargetKindV1::FunctionName,
        DecisionAnchorTargetKindV1::Pattern,
        value.clone(),
    )?;
    insert(
        candidates,
        DecisionAnchorTargetKindV1::FunctionName,
        DecisionAnchorTargetKindV1::FunctionName,
        value.clone(),
    )?;
    insert(
        candidates,
        DecisionAnchorTargetKindV1::FunctionName,
        DecisionAnchorTargetKindV1::QualifiedName,
        value,
    )
}

fn insert<C: CandidateCollection>(
    candidates: &mut C,
    source_kind: DecisionAnchorTargetKindV1,
    target_kind: DecisionAnchorTargetKindV1,
    value: String,
) -> Option<()> {
    source_kind.can_carry_forward(target_kind).then_some(())?;
    // A provider may report one identity through multiple approved fields
    // (for example `qualified_name`, a terminal `name`, and a `symbol` list).
    // They are equivalent representations of the same result, not ambiguous
    // independently returned candidates. The registry still rejects a
    // selector that later appears under a distinct root.
    candidates.insert_candidate(Candidate {
        kind: target_kind,
        provider_kind: source_kind,
        value,
    });
    Some(())
}
