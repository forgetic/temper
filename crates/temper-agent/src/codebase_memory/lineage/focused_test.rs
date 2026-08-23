//! Focused-test selector provenance from bounded typed result shapes.

use super::*;
use temper_protocol_activity::FocusedTestDiscoveryOutcomeV1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FocusedTestRecoveryState {
    TraversalReturnedEligible,
    TraversalReturnedEmpty,
    FallbackCompleted,
}

#[derive(Clone, Copy)]
pub(super) enum SelectorOrigin {
    CallerEvidenceResult,
    FocusedTestResult,
}

pub(super) struct FocusedTestDiscovery {
    pub(super) candidates: Option<BTreeSet<Candidate>>,
    pub(super) outcome: Option<FocusedTestDiscoveryOutcomeV1>,
    pub(super) is_traversal: bool,
}

pub(super) fn focused_test_discovery(
    correlation: &GraphCorrelationV1,
    input: &Value,
    caller_evidence_result: bool,
    is_fallback: bool,
    typed_parts: Option<&[McpToolResultPart]>,
) -> FocusedTestDiscovery {
    let candidates = provider_focused_test_candidates(typed_parts);
    let is_traversal = correlation.tool == GraphCorrelationToolV1::TracePath
        && input.get("mode").and_then(Value::as_str) == Some("calls")
        && input.get("direction").and_then(Value::as_str) == Some("inbound")
        && input.get("include_tests").and_then(Value::as_bool) == Some(true)
        && caller_evidence_result;
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

pub(super) fn provider_focused_test_candidates(
    typed_parts: Option<&[McpToolResultPart]>,
) -> Option<BTreeSet<Candidate>> {
    let mut candidates = BTreeMap::new();
    for part in typed_parts? {
        let value = match part {
            McpToolResultPart::StructuredContent(value) => {
                value.is_object().then(|| Some(value.clone()))?
            }
            McpToolResultPart::Content(block) => content_part_json(block)?,
        };
        if let Some(value) = value {
            collect_focused_test_result(&value, &mut candidates)?;
        }
    }
    let candidates = candidates.into_keys().collect::<BTreeSet<_>>();
    (candidates.len() <= MAX_RESULT_TARGETS).then_some(candidates)
}

fn collect_focused_test_result(
    value: &Value,
    candidates: &mut BTreeMap<Candidate, u8>,
) -> Option<()> {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_focused_test_result(value, candidates)?;
            }
        }
        Value::Object(values) => {
            if values
                .get("is_test")
                .or_else(|| values.get("isTest"))
                .is_some_and(|value| value == true)
            {
                collect_direct_symbol(values, candidates)?;
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
                            collect_focused_test_result(value, candidates)?;
                        }
                    }
                    "source_metadata" | "sourceMetadata" => {
                        collect_focused_test_result(value, candidates)?
                    }
                    _ => {}
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => return None,
    }
    (candidates.len() <= MAX_RESULT_TARGETS).then_some(())
}
