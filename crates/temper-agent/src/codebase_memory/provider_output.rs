//! Decode the release's columnar JSON before deriving typed graph evidence.

use serde_json::{Map, Value};

use crate::mcp::{McpToolCallResult, McpToolResultPart};

const MAX_NORMALIZED_BYTES: usize = 16 * 1024;

pub(super) fn uses_json_format(tool: &str) -> bool {
    matches!(
        tool,
        "search_graph" | "trace_path" | "search_code" | "get_architecture" | "detect_changes"
    )
}

pub(super) fn normalize(result: &mut McpToolCallResult) {
    if result.is_error || result.typed_parts.is_none() {
        return;
    }
    let Ok(mut value) = serde_json::from_str::<Value>(&result.text) else {
        return;
    };
    if !contains_table(&value) {
        return;
    }
    if !result.typed_parts.as_ref().is_some_and(|parts| {
        parts.iter().all(|part| match part {
            McpToolResultPart::StructuredContent(structured) => structured == &value,
            McpToolResultPart::Content(block) => {
                block.get("type").and_then(Value::as_str) == Some("text")
                    && block
                        .get("text")
                        .and_then(Value::as_str)
                        .and_then(|text| serde_json::from_str::<Value>(text).ok())
                        .as_ref()
                        == Some(&value)
            }
        })
    }) {
        result.typed_parts = None;
        return;
    }
    if expand_tables(&mut value, 0).is_none() {
        result.typed_parts = None;
        return;
    }
    let text = value.to_string();
    if text.len() > MAX_NORMALIZED_BYTES {
        result.typed_parts = None;
        return;
    }
    result.text = text;
    result.typed_parts = Some(vec![McpToolResultPart::StructuredContent(value)]);
}

fn contains_table(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.contains_key("cols") || object.values().any(contains_table),
        Value::Array(values) => values.iter().any(contains_table),
        _ => false,
    }
}

fn expand_tables(value: &mut Value, depth: usize) -> Option<()> {
    if depth > 16 {
        return None;
    }
    match value {
        Value::Object(object) => {
            if object.contains_key("cols") {
                let mut records = table_records(object)?;
                if object.contains_key("results") {
                    return None;
                }
                let returned = records.len();
                if object.get("search_mode").and_then(Value::as_str) == Some("bm25") {
                    records.retain(|record| !source_less_decorator(record));
                }
                let omitted = returned - records.len();
                if omitted > 0 {
                    // Keep provider totals/pagination intact: these are omitted
                    // metadata nodes, not evidence of an empty source query.
                    if object.contains_key("omitted_non_source_nodes") {
                        return None;
                    }
                    object.insert("omitted_non_source_nodes".to_string(), omitted.into());
                }
                object.remove("cols");
                object.remove("rows");
                object.remove("groups");
                object.insert("results".to_string(), Value::Array(records));
            }
            for child in object.values_mut() {
                expand_tables(child, depth + 1)?;
            }
        }
        Value::Array(values) => {
            for child in values {
                expand_tables(child, depth + 1)?;
            }
        }
        _ => {}
    }
    Some(())
}

fn source_less_decorator(value: &Value) -> bool {
    let Some(record) = value.as_object() else {
        return false;
    };
    // Released BM25 results include e.g. ["<decorator:test>", "Decorator", "", "", rank].
    // Only this closed, source-less record is metadata. Extra fields (including
    // nested source) and malformed real source paths must still face the guard.
    record.len() == 5
        && record.get("label").and_then(Value::as_str) == Some("Decorator")
        && record.get("file_path").and_then(Value::as_str) == Some("")
        && record.get("lines").and_then(Value::as_str) == Some("")
        && record.get("rank").is_some_and(Value::is_number)
        && record
            .get("qualified_name")
            .and_then(Value::as_str)
            .and_then(|name| name.strip_prefix("<decorator:")?.strip_suffix('>'))
            .is_some_and(|name| !name.is_empty())
}

fn table_records(object: &Map<String, Value>) -> Option<Vec<Value>> {
    let cols = object.get("cols")?.as_array()?;
    if cols.is_empty() {
        return None;
    }
    // Empty tables still need a valid schema before they can report absence.
    row_record(cols, &Value::Array(vec![Value::Null; cols.len()]))?;
    let mut records = Vec::new();
    let mut bytes = 0;
    if let Some(groups) = object.get("groups") {
        if object.contains_key("rows") {
            return None;
        }
        for group in groups.as_array()? {
            if !group
                .as_object()?
                .keys()
                .all(|key| matches!(key.as_str(), "qn_prefix" | "file" | "rows"))
            {
                return None;
            }
            let prefix = group.get("qn_prefix")?.as_str()?;
            if let Some(file) = group.get("file") {
                file.as_str()?;
            }
            for row in group.get("rows")?.as_array()? {
                let mut record = row_record(cols, row)?;
                let name = record.get("name")?.as_str()?;
                let qualified = if prefix.is_empty() {
                    name.to_string()
                } else {
                    format!("{prefix}.{name}")
                };
                insert_consistent(&mut record, "qualified_name", Value::String(qualified))?;
                if let Some(file) = group.get("file") {
                    insert_consistent(
                        &mut record,
                        "file_path",
                        Value::String(file.as_str()?.to_string()),
                    )?;
                }
                push_record(&mut records, &mut bytes, record)?;
            }
        }
    } else {
        for row in object.get("rows")?.as_array()? {
            push_record(&mut records, &mut bytes, row_record(cols, row)?)?;
        }
    }
    Some(records)
}

fn push_record(
    records: &mut Vec<Value>,
    bytes: &mut usize,
    record: Map<String, Value>,
) -> Option<()> {
    let record = Value::Object(record);
    *bytes = bytes.checked_add(record.to_string().len())?;
    if *bytes > MAX_NORMALIZED_BYTES {
        return None;
    }
    records.push(record);
    Some(())
}

fn row_record(cols: &[Value], row: &Value) -> Option<Map<String, Value>> {
    let cells = row.as_array()?;
    if cells.len() != cols.len() {
        return None;
    }
    let mut record = Map::new();
    for (col, cell) in cols.iter().zip(cells) {
        let key = match col.as_str()? {
            "qn" => "qualified_name",
            "file" => "file_path",
            key => key,
        };
        // Duplicate columns are ambiguous even when their values happen to agree.
        if record.insert(key.to_string(), cell.clone()).is_some() {
            return None;
        }
    }
    Some(record)
}

fn insert_consistent(record: &mut Map<String, Value>, key: &str, value: Value) -> Option<()> {
    if record.get(key).is_some_and(|existing| existing != &value) {
        return None;
    }
    record.insert(key.to_string(), value);
    Some(())
}

#[cfg(test)]
#[path = "tests/provider_output.rs"]
mod tests;
