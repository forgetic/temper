//! Focused-test selector provenance from bounded typed result shapes.

use super::*;
use temper_protocol_activity::FocusedTestDiscoveryOutcomeV1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FocusedTestRecoveryState {
    SemanticSearchReady,
    TraversalReturnedEligible,
    TraversalReturnedEmpty,
    FallbackPending,
    FallbackCompleted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SelectorOrigin {
    ImplementationEvidenceResult {
        traversal_evidence: ImplementationTraversalEvidence,
    },
    CallerTraversalResult,
    CallerEvidenceResult,
    FocusedTestResult,
    FocusedTestFallbackResult,
}

pub(super) struct FocusedTestDiscovery {
    pub(super) candidates: Option<BTreeSet<Candidate>>,
    pub(super) outcome: Option<FocusedTestDiscoveryOutcomeV1>,
    pub(super) is_traversal: bool,
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
    is_fallback: bool,
    typed_parts: Option<&[McpToolResultPart]>,
) -> FocusedTestDiscovery {
    let focused_candidates = provider_focused_test_candidates(typed_parts);
    let fallback_candidates = is_fallback
        .then(|| {
            let focused = focused_candidates.as_ref()?;
            if focused.has_test_classification {
                Some(focused.exact_source_candidates.clone())
            } else {
                provider_exact_source_candidates(typed_parts)
            }
        })
        .flatten();
    let is_traversal = correlation.tool == GraphCorrelationToolV1::TracePath
        && input.get("mode").and_then(Value::as_str) == Some("calls")
        && input.get("direction").and_then(Value::as_str) == Some("inbound")
        && input.get("include_tests").and_then(Value::as_bool) == Some(true)
        && caller_evidence_result;
    let candidates = if is_fallback {
        fallback_candidates
    } else if is_traversal {
        focused_candidates.map(|focused| focused.candidates)
    } else {
        None
    };
    let outcome = if correlation.tool == GraphCorrelationToolV1::SearchGraph
        && correlation.target_kind == GraphCorrelationTargetKindV1::GraphQuery
        && !is_fallback
    {
        provider_candidates(typed_parts).map(|candidates| {
            if candidates.is_empty() {
                FocusedTestDiscoveryOutcomeV1::NoEligibleSelector
            } else {
                FocusedTestDiscoveryOutcomeV1::EligibleSelectorReturned
            }
        })
    } else if is_traversal || is_fallback {
        candidates.as_ref().map(|candidates| {
            if candidates.is_empty() {
                FocusedTestDiscoveryOutcomeV1::NoEligibleSelector
            } else {
                FocusedTestDiscoveryOutcomeV1::EligibleSelectorReturned
            }
        })
    } else {
        None
    };
    FocusedTestDiscovery {
        candidates,
        outcome,
        is_traversal,
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
            SelectorOrigin::FocusedTestResult | SelectorOrigin::FocusedTestFallbackResult => {
                RecoverySelectorPurpose::FocusedTestSource
            }
        };
        let mut reference_candidates = candidates.iter().cloned().collect::<Vec<_>>();
        reference_candidates.sort_by_key(|candidate| {
            if candidate.kind == purpose.selector_kind()
                && candidate.provider_kind == purpose.selector_kind()
            {
                0
            } else if candidate.kind == candidate.provider_kind {
                1
            } else if candidate.kind == purpose.selector_kind() {
                2
            } else {
                3
            }
        });
        let mut marked = 0;
        for candidate in candidates {
            let selector = Selector {
                kind: candidate.kind,
                value: candidate.value,
            };
            let Some(Some(binding)) = self.selectors.get_mut(&selector) else {
                continue;
            };
            if binding.root_binding != root {
                continue;
            }
            match origin {
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
                }
                SelectorOrigin::CallerTraversalResult => binding.caller_traversal_result = true,
                SelectorOrigin::CallerEvidenceResult => binding.caller_evidence_result = true,
                SelectorOrigin::FocusedTestResult => binding.focused_test_result = true,
                SelectorOrigin::FocusedTestFallbackResult => {
                    binding.focused_test_result = true;
                    binding.focused_test_confirmation_required = true;
                }
            }
            marked += 1;
        }
        let reference_candidate = reference_candidates.into_iter().find_map(|candidate| {
            let selector_kind = purpose.selector_kind();
            let selector_value = match selector_kind {
                DecisionAnchorTargetKindV1::FunctionName => {
                    canonical_function_name(&candidate.value)?
                }
                DecisionAnchorTargetKindV1::QualifiedName => {
                    canonical_qualified_name(&candidate.value)
                        .or_else(|| canonical_function_name(&candidate.value))?
                }
                DecisionAnchorTargetKindV1::GraphQuery
                | DecisionAnchorTargetKindV1::Pattern
                | DecisionAnchorTargetKindV1::NamePattern
                | DecisionAnchorTargetKindV1::QualifiedNamePattern => return None,
            };
            let selector = Selector {
                kind: selector_kind,
                value: selector_value,
            };
            self.selectors
                .get(&selector)
                .and_then(Option::as_ref)
                .is_some_and(|binding| binding.root_binding == root)
                .then_some((candidate, selector))
        });
        if let Some((candidate, selector)) = reference_candidate {
            let key = RecoverySelectorKey {
                root_binding: root.to_string(),
                purpose,
            };
            let reference = format!(
                "{RECOVERY_SELECTOR_REFERENCE_PREFIX}{}",
                uuid::Uuid::new_v4()
            );
            let source_selector = (purpose == RecoverySelectorPurpose::ImplementationTrace)
                .then(|| {
                    canonical_qualified_name(&candidate.value)
                        .or_else(|| canonical_function_name(&candidate.value))
                        .map(|value| Selector {
                            kind: DecisionAnchorTargetKindV1::QualifiedName,
                            value,
                        })
                })
                .flatten()
                .filter(|selector| {
                    self.selectors
                        .get(selector)
                        .and_then(Option::as_ref)
                        .is_some_and(|binding| binding.root_binding == root)
                });
            self.recovery_reference_selectors.insert(
                reference.clone(),
                RecoverySelectorReference {
                    purpose,
                    selector,
                    provider_value: candidate.value.clone(),
                    source_selector: source_selector.clone(),
                    source_provider_value: source_selector.map(|_| candidate.value),
                },
            );
            self.recovery_references.insert(key, reference);
        }
        Some(marked)
    }

    pub(super) fn record_caller_evidence_ready(&mut self, root_binding: &str) {
        self.focused_test_recovery
            .entry(root_binding.to_string())
            .or_insert(FocusedTestRecoveryState::SemanticSearchReady);
    }

    pub(super) fn begin_focused_test_semantic_search(&mut self, root_binding: &str) {
        for binding in self.selectors.values_mut().flatten() {
            if binding.root_binding == root_binding {
                binding.focused_test_result = false;
                binding.focused_test_confirmation_required = false;
            }
        }
        let reference_key = RecoverySelectorKey {
            root_binding: root_binding.to_string(),
            purpose: RecoverySelectorPurpose::FocusedTestSource,
        };
        if let Some(reference) = self.recovery_references.remove(&reference_key) {
            self.recovery_reference_selectors.remove(&reference);
        }
        self.focused_test_recovery.insert(
            root_binding.to_string(),
            FocusedTestRecoveryState::FallbackPending,
        );
    }

    pub(super) fn record_registered_focused_test_recovery(
        &mut self,
        root_binding: &str,
        is_traversal: bool,
        is_fallback: bool,
        had_candidates: bool,
        marked_candidates: Option<usize>,
        outcome: Option<FocusedTestDiscoveryOutcomeV1>,
    ) -> Option<FocusedTestDiscoveryOutcomeV1> {
        let outcome = if (is_traversal || is_fallback) && had_candidates {
            Some(if marked_candidates.unwrap_or_default() == 0 {
                FocusedTestDiscoveryOutcomeV1::NoEligibleSelector
            } else {
                FocusedTestDiscoveryOutcomeV1::EligibleSelectorReturned
            })
        } else {
            outcome
        };
        self.record_focused_test_recovery(root_binding, is_traversal, is_fallback, outcome);
        outcome
    }

    pub(super) fn record_focused_test_recovery(
        &mut self,
        root_binding: &str,
        is_traversal: bool,
        is_fallback: bool,
        outcome: Option<FocusedTestDiscoveryOutcomeV1>,
    ) {
        if is_traversal {
            if let Some(outcome) = outcome {
                self.focused_test_recovery.insert(
                    root_binding.to_string(),
                    match outcome {
                        FocusedTestDiscoveryOutcomeV1::EligibleSelectorReturned => {
                            FocusedTestRecoveryState::TraversalReturnedEligible
                        }
                        FocusedTestDiscoveryOutcomeV1::NoEligibleSelector => {
                            FocusedTestRecoveryState::TraversalReturnedEmpty
                        }
                    },
                );
            }
        }
        if is_fallback {
            self.focused_test_recovery.insert(
                root_binding.to_string(),
                FocusedTestRecoveryState::FallbackCompleted,
            );
        }
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
            let test_fields = ["is_test", "isTest"]
                .into_iter()
                .filter_map(|field| values.get(field))
                .collect::<Vec<_>>();
            if !test_fields.is_empty() {
                *has_test_classification = true;
                let mut classifications = test_fields
                    .into_iter()
                    .map(Value::as_bool)
                    .collect::<Option<BTreeSet<_>>>()?;
                if classifications.len() != 1 {
                    return None;
                }
                if classifications.pop_first()? {
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

fn provider_exact_source_candidates(
    typed_parts: Option<&[McpToolResultPart]>,
) -> Option<BTreeSet<Candidate>> {
    let mut candidates = BTreeSet::new();
    for part in typed_parts? {
        let value = match part {
            McpToolResultPart::StructuredContent(value) => {
                value.is_object().then(|| Some(value.clone()))?
            }
            McpToolResultPart::Content(block) => content_part_json(block)?,
        };
        if let Some(value) = value {
            collect_exact_source_result(&value, &mut candidates)?;
        }
    }
    (candidates.len() <= MAX_RESULT_TARGETS).then_some(candidates)
}

fn collect_exact_source_result(value: &Value, candidates: &mut BTreeSet<Candidate>) -> Option<()> {
    match value {
        Value::Array(values) => {
            for value in values {
                match value {
                    Value::String(value) => insert_exact_source_candidate(candidates, value)?,
                    _ => collect_exact_source_result(value, candidates)?,
                }
            }
        }
        Value::Object(values) => {
            collect_direct_symbol(values, &mut BTreeMap::new())?;
            collect_direct_exact_source_candidates(values, candidates)?;
            for (field, value) in values {
                match field.as_str() {
                    "results"
                    | "semantic_results"
                    | "semanticResults"
                    | "callers"
                    | "caller_list"
                    | "callerList"
                    | "caller_functions"
                    | "callerFunctions"
                    | "callees"
                    | "callee_list"
                    | "calleeList"
                    | "callee_functions"
                    | "calleeFunctions"
                    | "symbols"
                    | "short_symbols"
                    | "shortSymbols"
                    | "related_source_references"
                    | "relatedSourceReferences"
                    | "related_source_refs"
                    | "relatedSourceRefs"
                    | "related_sources"
                    | "relatedSources" => {
                        if !value.is_u64() {
                            collect_exact_source_result(value, candidates)?;
                        }
                    }
                    "next_target" | "nextTarget" | "function" | "source_metadata"
                    | "sourceMetadata" => collect_exact_source_result(value, candidates)?,
                    "symbol" if value.is_object() => {
                        collect_exact_source_result(value, candidates)?
                    }
                    _ => {}
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => return None,
    }
    (candidates.len() <= MAX_RESULT_TARGETS).then_some(())
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
            value: identity,
        });
    }
    Some(())
}

fn canonical_source_selector(value: &str) -> Option<String> {
    canonical_qualified_name(value).or_else(|| canonical_function_name(value))
}

fn insert_exact_source_candidate(candidates: &mut BTreeSet<Candidate>, value: &str) -> Option<()> {
    candidates.insert(Candidate {
        kind: DecisionAnchorTargetKindV1::QualifiedName,
        provider_kind: DecisionAnchorTargetKindV1::QualifiedName,
        value: canonical_source_selector(value)?,
    });
    Some(())
}
