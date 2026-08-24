//! Focused-test selector provenance from bounded typed result shapes.

use super::*;
use temper_protocol_activity::FocusedTestDiscoveryOutcomeV1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FocusedTestRecoveryState {
    TraversalReturnedEligible,
    TraversalReturnedEmpty,
    FallbackPending,
    FallbackCompleted,
}

#[derive(Clone, Copy)]
pub(super) enum SelectorOrigin {
    ImplementationEvidenceResult,
    CallerTraversalResult,
    CallerEvidenceResult,
    FocusedTestResult,
}

pub(super) struct FocusedTestDiscovery {
    pub(super) candidates: Option<BTreeSet<Candidate>>,
    pub(super) outcome: Option<FocusedTestDiscoveryOutcomeV1>,
    pub(super) is_traversal: bool,
}

struct ProviderFocusedTestCandidates {
    candidates: BTreeSet<Candidate>,
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
            let candidates = if focused.has_test_classification {
                focused.candidates.clone()
            } else {
                provider_candidates(typed_parts)?
            };
            Some(single_exact_identity(candidates))
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
    let outcome = if correlation.tool == GraphCorrelationToolV1::SearchGraph && !is_fallback {
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
    ) -> Option<()> {
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
                SelectorOrigin::ImplementationEvidenceResult => {
                    binding.implementation_evidence_result = true
                }
                SelectorOrigin::CallerTraversalResult => binding.caller_traversal_result = true,
                SelectorOrigin::CallerEvidenceResult => binding.caller_evidence_result = true,
                SelectorOrigin::FocusedTestResult => binding.focused_test_result = true,
            }
        }
        Some(())
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

fn provider_focused_test_candidates(
    typed_parts: Option<&[McpToolResultPart]>,
) -> Option<ProviderFocusedTestCandidates> {
    let mut candidates = BTreeMap::new();
    let mut has_test_classification = false;
    for part in typed_parts? {
        let value = match part {
            McpToolResultPart::StructuredContent(value) => {
                value.is_object().then(|| Some(value.clone()))?
            }
            McpToolResultPart::Content(block) => content_part_json(block)?,
        };
        if let Some(value) = value {
            collect_focused_test_result(&value, &mut candidates, &mut has_test_classification)?;
        }
    }
    let candidates = candidates.into_keys().collect::<BTreeSet<_>>();
    (candidates.len() <= MAX_RESULT_TARGETS).then_some(ProviderFocusedTestCandidates {
        candidates,
        has_test_classification,
    })
}

fn collect_focused_test_result(
    value: &Value,
    candidates: &mut BTreeMap<Candidate, u8>,
    has_test_classification: &mut bool,
) -> Option<()> {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_focused_test_result(value, candidates, has_test_classification)?;
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
                    "results" | "callers" | "caller_list" | "callerList" | "caller_functions"
                    | "callerFunctions" | "callees" | "callee_list" | "calleeList"
                    | "callee_functions" | "calleeFunctions" | "symbols" | "short_symbols"
                    | "shortSymbols" => {
                        if !value.is_u64() {
                            collect_focused_test_result(
                                value,
                                candidates,
                                has_test_classification,
                            )?;
                        }
                    }
                    "source_metadata" | "sourceMetadata" => {
                        collect_focused_test_result(value, candidates, has_test_classification)?
                    }
                    _ => {}
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => return None,
    }
    (candidates.len() <= MAX_RESULT_TARGETS).then_some(())
}

/// A semantic fallback has one closed focused-test purpose, so an unclassified
/// provider result is useful only when all of its supported selector forms
/// identify one exact symbol. Multiple identities settle as an empty result;
/// provider labels, paths, ranking, and prose never participate in selection.
fn single_exact_identity(candidates: BTreeSet<Candidate>) -> BTreeSet<Candidate> {
    if candidates.is_empty() {
        return candidates;
    }
    let qualified = candidates
        .iter()
        .filter_map(|candidate| canonical_qualified_name(&candidate.value))
        .collect::<BTreeSet<_>>();
    let short = candidates
        .iter()
        .filter_map(|candidate| canonical_function_name(&candidate.value))
        .collect::<BTreeSet<_>>();
    let exact = match qualified.len() {
        0 => short.len() == 1,
        1 => {
            short.len() == 1
                && qualified
                    .first()
                    .and_then(|identity| terminal_function_name(identity))
                    .as_ref()
                    == short.first()
        }
        _ => false,
    };
    if exact { candidates } else { BTreeSet::new() }
}
