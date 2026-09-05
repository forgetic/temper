//! Focused-test selector provenance from bounded typed result shapes.

use super::*;
use temper_protocol_activity::FocusedTestDiscoveryOutcomeV1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SelectorOrigin {
    ImplementationEvidenceResult {
        traversal_evidence: ImplementationTraversalEvidence,
    },
    CallerTraversalResult,
    CallerEvidenceResult,
    FocusedTestResult,
}

pub(super) struct FocusedTestDiscovery {
    pub(super) candidates: Option<BTreeSet<Candidate>>,
    pub(super) outcome: Option<FocusedTestDiscoveryOutcomeV1>,
}

struct ProviderFocusedTestCandidates {
    candidates: BTreeSet<Candidate>,
    exact_source_candidates: BTreeSet<Candidate>,
    has_test_classification: bool,
}

pub(super) fn focused_test_discovery(
    correlation: &GraphCorrelationV1,
    input: &Value,
    caller_evidence_result: bool,
    typed_parts: Option<&[McpToolResultPart]>,
) -> FocusedTestDiscovery {
    let focused_candidates = provider_focused_test_candidates(typed_parts);
    let is_traversal = correlation.tool == GraphCorrelationToolV1::TracePath
        && input.get("mode").and_then(Value::as_str) == Some("calls")
        && input.get("direction").and_then(Value::as_str) == Some("inbound")
        && input.get("include_tests").and_then(Value::as_bool) == Some(true)
        && caller_evidence_result;
    let is_initial_search = correlation.tool == GraphCorrelationToolV1::SearchGraph
        && correlation.target_kind == GraphCorrelationTargetKindV1::GraphQuery
        && !caller_evidence_result;
    let candidates = if is_traversal {
        focused_candidates.map(|focused| focused.candidates)
    } else if is_initial_search {
        focused_candidates.and_then(|focused| {
            focused
                .has_test_classification
                .then_some(focused.exact_source_candidates)
        })
    } else {
        None
    };
    let outcome = (is_initial_search || is_traversal)
        .then(|| {
            candidates.as_ref().map(|candidates| {
                if candidates.is_empty() {
                    FocusedTestDiscoveryOutcomeV1::NoEligibleSelector
                } else {
                    FocusedTestDiscoveryOutcomeV1::EligibleSelectorReturned
                }
            })
        })
        .flatten();
    FocusedTestDiscovery {
        candidates,
        outcome,
    }
}

impl DecisionAnchorLineages {
    pub(super) fn mark_candidates(
        &mut self,
        root: &str,
        candidates: BTreeSet<Candidate>,
        origin: SelectorOrigin,
    ) -> Option<usize> {
        let purpose = match origin {
            SelectorOrigin::ImplementationEvidenceResult { .. } => {
                RecoverySelectorPurpose::ImplementationTrace
            }
            SelectorOrigin::CallerTraversalResult => RecoverySelectorPurpose::CallerSource,
            SelectorOrigin::CallerEvidenceResult => RecoverySelectorPurpose::CallerTestTraversal,
            SelectorOrigin::FocusedTestResult => RecoverySelectorPurpose::FocusedTestSource,
        };
        let mut marked = 0;
        for candidate in &candidates {
            let selector = Selector {
                kind: candidate.kind,
                value: candidate.value.clone(),
            };
            let mark = |binding: &mut SelectorBinding| match origin {
                SelectorOrigin::ImplementationEvidenceResult { traversal_evidence } => {
                    let already_recorded = binding.implementation_evidence_result;
                    binding.implementation_evidence_result = true;
                    binding.implementation_traversal_readiness = match (
                        already_recorded,
                        binding.implementation_traversal_readiness,
                        traversal_evidence,
                    ) {
                        (_, _, ImplementationTraversalEvidence::CallerIdentity) => {
                            ImplementationTraversalReadiness::Ready
                        }
                        (false, _, ImplementationTraversalEvidence::Ready) => {
                            ImplementationTraversalReadiness::Ready
                        }
                        (false, _, ImplementationTraversalEvidence::Partial) => {
                            ImplementationTraversalReadiness::Partial
                        }
                        (true, ImplementationTraversalReadiness::Ready, _) => {
                            ImplementationTraversalReadiness::Ready
                        }
                        (true, ImplementationTraversalReadiness::RecheckPending, _) => {
                            ImplementationTraversalReadiness::RecheckExhausted
                        }
                        (true, readiness, _) => readiness,
                    };
                    if candidate.kind != DecisionAnchorTargetKindV1::QualifiedName {
                        binding.recovery_reference_required = false;
                    }
                }
                SelectorOrigin::CallerTraversalResult => binding.caller_traversal_result = true,
                SelectorOrigin::CallerEvidenceResult => {
                    binding.caller_evidence_result = true;
                    binding.recovery_reference_required = false;
                }
                SelectorOrigin::FocusedTestResult => binding.focused_test_result = true,
            };
            let Some(binding) = self
                .root_selectors
                .get_mut(&(root.to_string(), selector.clone()))
            else {
                continue;
            };
            mark(binding);
            if let Some(Some(binding)) = self.selectors.get_mut(&selector) {
                if binding.root_binding == root {
                    mark(binding);
                }
            }
            marked += 1;
        }
        let ordered = candidates.iter().cloned().collect::<Vec<_>>();
        self.replace_recovery_references(root, purpose, &ordered, false);
        Some(marked)
    }

    pub(super) fn record_registered_focused_test_candidates(
        &mut self,
        had_candidates: bool,
        marked_candidates: Option<usize>,
        outcome: Option<FocusedTestDiscoveryOutcomeV1>,
    ) -> Option<FocusedTestDiscoveryOutcomeV1> {
        let outcome = if had_candidates {
            Some(if marked_candidates.unwrap_or_default() == 0 {
                FocusedTestDiscoveryOutcomeV1::NoEligibleSelector
            } else {
                FocusedTestDiscoveryOutcomeV1::EligibleSelectorReturned
            })
        } else {
            outcome
        };
        outcome
    }
}

pub(super) fn source_confirms_exact_test(
    selector: &Selector,
    typed_parts: Option<&[McpToolResultPart]>,
) -> bool {
    provider_focused_test_candidates(typed_parts).is_some_and(|focused| {
        focused.has_test_classification
            && focused.exact_source_candidates.contains(&Candidate {
                kind: selector.kind,
                provider_kind: DecisionAnchorTargetKindV1::QualifiedName,
                value: selector.value.clone(),
                provider_value: selector.value.clone(),
                provider_result_order: 0,
                provider_result_is_test: true,
            })
    })
}

fn provider_focused_test_candidates(
    typed_parts: Option<&[McpToolResultPart]>,
) -> Option<ProviderFocusedTestCandidates> {
    let mut candidates = BTreeMap::new();
    let mut exact_source_candidates = BTreeSet::new();
    let mut has_test_classification = false;
    for part in typed_parts? {
        let value = match part {
            McpToolResultPart::StructuredContent(value) => {
                value.is_object().then(|| Some(value.clone()))?
            }
            McpToolResultPart::Content(block) => content_part_json(block)?,
        };
        if let Some(value) = value {
            collect_focused_test_result(
                &value,
                &mut candidates,
                &mut exact_source_candidates,
                &mut has_test_classification,
            )?;
        }
    }
    let candidates = candidates.into_keys().collect::<BTreeSet<_>>();
    (candidates.len() <= MAX_RESULT_TARGETS && exact_source_candidates.len() <= MAX_RESULT_TARGETS)
        .then_some(ProviderFocusedTestCandidates {
            candidates,
            exact_source_candidates,
            has_test_classification,
        })
}

fn collect_focused_test_result(
    value: &Value,
    candidates: &mut BTreeMap<Candidate, u8>,
    exact_source_candidates: &mut BTreeSet<Candidate>,
    has_test_classification: &mut bool,
) -> Option<()> {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_focused_test_result(
                    value,
                    candidates,
                    exact_source_candidates,
                    has_test_classification,
                )?;
            }
        }
        Value::Object(values) => {
            let (has_classification, is_test) = provider_record_test_classification(values)?;
            if has_classification {
                *has_test_classification = true;
                if is_test {
                    collect_direct_symbol(values, candidates)?;
                    collect_direct_exact_source_candidates(values, exact_source_candidates)?;
                }
            }
            for (field, value) in values {
                match field.as_str() {
                    "related_source_references"
                    | "relatedSourceReferences"
                    | "related_source_refs"
                    | "relatedSourceRefs"
                    | "related_sources"
                    | "relatedSources" => collect_reference_list(value, candidates)?,
                    "results" | "semantic_results" | "semanticResults" | "callers"
                    | "caller_list" | "callerList" | "caller_functions" | "callerFunctions"
                    | "callees" | "callee_list" | "calleeList" | "callee_functions"
                    | "calleeFunctions" | "symbols" | "short_symbols" | "shortSymbols" => {
                        if !value.is_u64() {
                            collect_focused_test_result(
                                value,
                                candidates,
                                exact_source_candidates,
                                has_test_classification,
                            )?;
                        }
                    }
                    "source_metadata" | "sourceMetadata" => collect_focused_test_result(
                        value,
                        candidates,
                        exact_source_candidates,
                        has_test_classification,
                    )?,
                    _ => {}
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => return None,
    }
    (candidates.len() <= MAX_RESULT_TARGETS && exact_source_candidates.len() <= MAX_RESULT_TARGETS)
        .then_some(())
}

pub(super) fn provider_record_test_classification(
    values: &serde_json::Map<String, Value>,
) -> Option<(bool, bool)> {
    let test_fields = ["is_test", "isTest"]
        .into_iter()
        .filter_map(|field| values.get(field))
        .collect::<Vec<_>>();
    let path_classifications = ["file_path", "filePath", "source_path", "sourcePath"]
        .into_iter()
        .filter_map(|field| values.get(field))
        .map(|value| value.as_str().map(provider_classifies_test_path))
        .collect::<Option<BTreeSet<_>>>()?;
    let explicit_classification = if test_fields.is_empty() {
        None
    } else {
        let classifications = test_fields
            .into_iter()
            .map(Value::as_bool)
            .collect::<Option<BTreeSet<_>>>()?;
        if classifications.len() != 1 {
            return None;
        }
        classifications.first().copied()
    };
    let path_classification = if path_classifications.is_empty() {
        None
    } else {
        if path_classifications.len() != 1 {
            return None;
        }
        path_classifications.first().copied()
    };
    if explicit_classification == Some(false) && path_classification == Some(true) {
        return None;
    }
    Some((
        explicit_classification.is_some() || path_classification == Some(true),
        explicit_classification.unwrap_or(false) || path_classification == Some(true),
    ))
}

fn provider_classifies_test_path(value: &str) -> bool {
    let normalized = value.replace('\\', "/");
    !normalized.starts_with('/')
        && normalized
            .split('/')
            .all(|component| !matches!(component, "" | "." | ".."))
        && (normalized.starts_with("tests/") || normalized.contains("/tests/"))
}

fn collect_direct_exact_source_candidates(
    values: &serde_json::Map<String, Value>,
    candidates: &mut BTreeSet<Candidate>,
) -> Option<()> {
    let qualified = one_symbol_field(values, &["qualified_name", "qualifiedName"])?;
    let function = one_symbol_field(
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
    )?;
    let display_name = if qualified.is_some() || function.is_some() {
        one_symbol_field(values, &["name"])?
    } else {
        None
    };

    let qualified_identity = qualified.as_deref().and_then(canonical_source_selector);
    let function_identity = function.as_deref().and_then(canonical_source_selector);
    let display_identity = match display_name.as_deref() {
        Some(value) => Some(canonical_function_name(value)?),
        None => None,
    };
    let terminal = qualified_identity
        .as_deref()
        .or(function_identity.as_deref())
        .and_then(terminal_function_name);
    if let (Some(terminal), Some(display)) = (terminal.as_ref(), display_identity.as_ref()) {
        (terminal == display).then_some(())?;
    }

    for identity in [qualified_identity, function_identity, display_identity]
        .into_iter()
        .flatten()
    {
        candidates.insert(Candidate {
            kind: DecisionAnchorTargetKindV1::QualifiedName,
            provider_kind: DecisionAnchorTargetKindV1::QualifiedName,
            provider_value: identity.clone(),
            value: identity,
            provider_result_order: 0,
            provider_result_is_test: true,
        });
    }
    Some(())
}

fn canonical_source_selector(value: &str) -> Option<String> {
    canonical_qualified_name(value).or_else(|| canonical_function_name(value))
}
