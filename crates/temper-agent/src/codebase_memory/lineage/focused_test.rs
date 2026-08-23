//! Focused-test selector provenance from bounded typed result shapes.

use super::*;

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
