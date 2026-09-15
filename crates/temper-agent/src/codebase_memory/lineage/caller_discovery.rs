//! Narrow negative evidence from a complete production-only caller traversal.

use super::*;

impl DecisionAnchorLineages {
    pub(super) fn reports_no_production_callers(
        &self,
        root: &str,
        input: &Value,
        typed_parts: Option<&[McpToolResultPart]>,
    ) -> bool {
        let selected = self.selected_trace_source(root, input);
        selected.is_some_and(|selected| complete_report_parts(input, typed_parts, &selected))
    }

    fn selected_trace_source(&self, root: &str, input: &Value) -> Option<String> {
        let selector =
            self.selector_for_input(GraphCorrelationTargetKindV1::FunctionName, input)?;
        let key = RecoverySelectorKey {
            root_binding: root.to_string(),
            purpose: RecoverySelectorPurpose::ImplementationTrace,
        };
        let references = self.recovery_references.get(&key)?;
        let mut sources = references.iter().filter_map(|reference| {
            let reference = self.recovery_reference_selectors.get(reference)?;
            (reference
                .selector(DecisionAnchorTargetKindV1::FunctionName)
                .as_ref()
                == Some(&selector))
            .then_some(reference.source_selector.as_ref()?.value.as_str())
        });
        let selected = canonical_qualified_name(sources.next()?)?;
        sources
            .all(|source| canonical_qualified_name(source).as_ref() == Some(&selected))
            .then_some(selected)
    }
}

fn complete_report_parts(
    input: &Value,
    typed_parts: Option<&[McpToolResultPart]>,
    selected: &str,
) -> bool {
    let function = input.get("function_name").and_then(Value::as_str);
    let matches_selected = function.is_some_and(|function| {
        canonical_qualified_name(function).as_deref() == Some(selected)
            || canonical_function_name(selected).as_deref() == Some(function)
    });
    if !matches_selected
        || !input
            .get("direction")
            .is_none_or(|value| value == "inbound")
        || !input.get("mode").is_none_or(|value| value == "calls")
        || !input
            .get("include_tests")
            .is_none_or(|value| value == false)
        || !input.get("edge_types").is_none_or(|value| {
            value
                .as_array()
                .is_some_and(|edges| edges.len() == 1 && edges[0] == "CALLS")
        })
        || !["depth", "limit"].iter().all(|field| {
            input
                .get(*field)
                .is_none_or(|value| value.as_u64().is_some_and(|value| value > 0))
        })
        || input.get("cursor").is_some()
        || input.get("parameter_name").is_some()
    {
        return false;
    }
    let Some(parts) = typed_parts.filter(|parts| !parts.is_empty()) else {
        return false;
    };
    parts.iter().all(|part| {
        let value = match part {
            McpToolResultPart::StructuredContent(value) => Some(value.clone()),
            McpToolResultPart::Content(block) => content_part_json(block).flatten(),
        };
        value.is_some_and(|value| complete_zero_report(&value, selected))
    })
}

fn complete_zero_report(value: &Value, selected: &str) -> bool {
    let Some(record) = value.as_object() else {
        return false;
    };
    if record.get("callers_total").and_then(Value::as_u64) != Some(0)
        || !record
            .get("callers")
            .and_then(reference_records)
            .is_some_and(Vec::is_empty)
        || record.get("direction").and_then(Value::as_str) != Some("inbound")
        || !record
            .get("function")
            .and_then(Value::as_str)
            .and_then(canonical_qualified_name)
            .is_some_and(|reported| reported == selected)
    {
        return false;
    }
    // Unknown metadata cannot establish that this is the complete caller set.
    record.iter().all(|(field, value)| match field.as_str() {
        "function" | "direction" | "callers_total" | "callers" => true,
        "mode" => value == "calls",
        "include_tests" | "truncated" | "has_more" => value == false,
        "next" | "next_cursor" | "nextCursor" => value.is_null(),
        _ => false,
    })
}
