//! Closed identity metadata shared with the provider-neutral lineage decoder.

use serde_json::{Map, Value};

pub(super) const PATH_FIELDS: [&str; 6] = [
    "file_path",
    "filePath",
    "source_path",
    "sourcePath",
    "file",
    "path",
];

pub(super) fn retain_fields(object: &mut Map<String, Value>) -> Option<()> {
    object.retain(|key, value| safe_field(key, value));
    for (key, value) in object.iter() {
        let valid = match key.as_str() {
            "source_metadata" | "sourceMetadata" => value.is_object(),
            "related_source_references"
            | "relatedSourceReferences"
            | "related_source_refs"
            | "relatedSourceRefs"
            | "related_sources"
            | "relatedSources" => reference_list(value),
            "callers" | "caller_list" | "callerList" | "caller_functions" | "callerFunctions"
            | "callees" | "callee_list" | "calleeList" | "callee_functions" | "calleeFunctions"
            | "symbols" | "short_symbols" | "shortSymbols" => {
                value.is_u64() || reference_list(value)
            }
            "next_target" | "nextTarget" | "function" | "symbol" => reference(value),
            "semantic_results" | "semanticResults" => value
                .as_array()
                .is_some_and(|values| values.iter().all(Value::is_object)),
            _ => true,
        };
        if !valid {
            return None;
        }
    }
    Some(())
}

fn reference(value: &Value) -> bool {
    value.is_string() || value.is_object()
}

fn reference_list(value: &Value) -> bool {
    let records = value.as_array().or_else(|| {
        let object = value.as_object()?;
        (object.len() == 1)
            .then(|| object.get("results")?.as_array())
            .flatten()
    });
    records.is_some_and(|values| values.iter().all(reference))
}

fn safe_field(key: &str, value: &Value) -> bool {
    // Preserve only the decoder's closed identity containers. The source guard
    // recursively inspects them before any nested source can reach lineage or
    // presentation. Unknown text and sidecars are still discarded.
    PATH_FIELDS.contains(&key)
        || matches!(
            key,
            "source"
                | "context"
                | "snippet"
                | "code"
                | "content"
                | "qualified_name"
                | "qualifiedName"
                | "function_name"
                | "functionName"
                | "short_symbol"
                | "shortSymbol"
                | "short_name"
                | "shortName"
                | "symbol_name"
                | "symbolName"
                | "symbol"
                | "name"
                | "label"
                | "language"
                | "kind"
                | "type"
                | "project"
                | "lines"
                | "match_method"
                | "results"
                | "semantic_results"
                | "semanticResults"
                | "nodes"
                | "matches"
                | "snippets"
                | "raw_matches"
                | "files"
                | "directories"
                | "caller_names"
                | "callee_names"
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
                | "relatedSources"
                | "next_target"
                | "nextTarget"
                | "function"
                | "source_metadata"
                | "sourceMetadata"
                | "start_line"
                | "end_line"
                | "source_start"
                | "context_start"
                | "source_truncated"
                | "source_clipped"
        )
        || value.is_number()
        || value.is_boolean()
        || value.is_null()
}
