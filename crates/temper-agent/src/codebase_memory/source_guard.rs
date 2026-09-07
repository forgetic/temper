//! Verify source at the presentation boundary against the selected checkout.
//! This proves returned bytes, not a shared graph/coverage snapshot or epoch.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use serde_json::{Map, Value};

use super::scope::WorkspaceScope;
use crate::mcp::{McpToolCallResult, McpToolResultPart};

const MAX_FILE_BYTES: u64 = 1024 * 1024;
const MAX_TOTAL_BYTES: usize = 16 * 1024 * 1024;

pub(super) fn verify(
    scope: &WorkspaceScope,
    project: &str,
    tool: &str,
    result: &mut McpToolCallResult,
) -> bool {
    if result.is_error || !matches!(tool, "get_code_snippet" | "search_code" | "search_graph") {
        return true;
    }
    let Some(root) = scope
        .projects
        .iter()
        .find(|candidate| candidate.actual_project() == project)
        .map(|project| project.root.as_path())
    else {
        return false;
    };
    if tool == "get_code_snippet"
        && !serde_json::from_str::<Value>(&result.text)
            .ok()
            .as_ref()
            .is_some_and(has_source)
    {
        return false;
    }
    verify_at_root(root, result)
}

fn verify_at_root(root: &Path, result: &mut McpToolCallResult) -> bool {
    let Ok(mut value) = serde_json::from_str::<Value>(&result.text) else {
        return false;
    };
    if result.text.len() > super::MAX_CODEBASE_MEMORY_OUTPUT_BYTES
        || !consistent_parts(result, &value)
    {
        return false;
    }
    let Some(object) = value.as_object() else {
        return false;
    };
    let search_shape = ["results", "nodes", "matches", "files"]
        .iter()
        .any(|key| object.get(*key).is_some_and(Value::is_array));
    if !has_source(&value) && !search_shape {
        return false;
    }
    let mut total = 0;
    if check_value(root, &mut value, None, &mut total, 0).is_none() || (total == 0 && !search_shape)
    {
        return false;
    }
    // Rebuild from the single verified representation: no raw multipart sidecar
    // can survive even when another part carried a valid local source fragment.
    result.text = value.to_string();
    result.typed_parts = Some(vec![McpToolResultPart::StructuredContent(value)]);
    true
}

fn has_source(value: &Value) -> bool {
    match value {
        Value::Object(object) => {
            ["source", "context", "snippet", "code", "content"]
                .iter()
                .any(|key| object.get(*key).is_some_and(Value::is_string))
                || object.values().any(has_source)
        }
        Value::Array(values) => values.iter().any(has_source),
        _ => false,
    }
}

fn safe_field(key: &str, value: &Value) -> bool {
    // Reconstruct only known release metadata and verified source. Arbitrary
    // text/message properties and nested sidecars never pass through raw.
    matches!(
        key,
        "source"
            | "context"
            | "snippet"
            | "code"
            | "content"
            | "file_path"
            | "file"
            | "path"
            | "qualified_name"
            | "name"
            | "label"
            | "language"
            | "kind"
            | "type"
            | "project"
            | "lines"
            | "match_method"
            | "results"
            | "nodes"
            | "matches"
            | "snippets"
            | "raw_matches"
            | "files"
            | "directories"
            | "caller_names"
            | "callee_names"
            | "start_line"
            | "end_line"
            | "source_start"
            | "context_start"
            | "source_truncated"
            | "source_clipped"
    ) || value.is_number()
        || value.is_boolean()
        || value.is_null()
}

fn consistent_parts(result: &McpToolCallResult, value: &Value) -> bool {
    result.typed_parts.as_ref().is_some_and(|parts| {
        !parts.is_empty()
            && parts.iter().all(|part| match part {
                McpToolResultPart::StructuredContent(structured) => structured == value,
                McpToolResultPart::Content(block) => {
                    block.get("type").and_then(Value::as_str) == Some("text")
                        && block
                            .get("text")
                            .and_then(Value::as_str)
                            .and_then(|text| serde_json::from_str::<Value>(text).ok())
                            .as_ref()
                            == Some(value)
                }
            })
    })
}

fn check_value(
    root: &Path,
    value: &mut Value,
    inherited: Option<&Map<String, Value>>,
    total: &mut usize,
    depth: usize,
) -> Option<()> {
    if depth > 16 {
        return None;
    }
    match value {
        Value::Object(object) => {
            // Stored signatures/docstrings have no exact source range contract.
            // Retain symbol/path metadata; omit these optional source-derived
            // fields rather than represent another checkout's stored text.
            object.retain(|key, value| safe_field(key, value));
            for key in ["results", "nodes", "snippets"] {
                if object.get(key).is_some_and(|value| {
                    !value
                        .as_array()
                        .is_some_and(|values| values.iter().all(Value::is_object))
                }) {
                    return None;
                }
            }
            if object.get("matches").is_some_and(|value| {
                value.as_array().is_some_and(|values| {
                    values
                        .iter()
                        .any(|value| !value.is_object() && !value.is_number())
                })
            }) {
                return None;
            }
            for key in ["source_truncated", "source_clipped"] {
                if object.get(key).is_some_and(|value| !value.is_boolean()) {
                    return None;
                }
            }
            if ["file_path", "file", "path"]
                .iter()
                .any(|key| object.contains_key(*key))
                && file_path(object).is_none()
            {
                return None;
            }
            let binding = if file_path(object).is_some() {
                Some(object.clone())
            } else {
                inherited.cloned()
            };
            for key in ["source", "context", "snippet", "code", "content"] {
                if let Some(fragment) = object.get(key) {
                    if let Some(source) = fragment.as_str() {
                        check_fragment(root, binding.as_ref()?, object, key, source, total)?;
                    } else if fragment.is_object() && !has_source(fragment) {
                        return None;
                    } else if !fragment.is_object() && !fragment.is_null() {
                        return None;
                    }
                }
            }
            for child in object.values_mut() {
                check_value(root, child, binding.as_ref(), total, depth + 1)?;
            }
        }
        Value::Array(values) => {
            for child in values {
                check_value(root, child, inherited, total, depth + 1)?;
            }
        }
        _ => {}
    }
    Some(())
}

fn file_path(object: &Map<String, Value>) -> Option<&str> {
    let mut found = None;
    for key in ["file_path", "file", "path"] {
        if let Some(value) = object.get(key) {
            let path = value.as_str().filter(|path| !path.is_empty())?;
            if found.is_some_and(|previous| previous != path) {
                return None;
            }
            found = Some(path);
        }
    }
    found
}

fn check_fragment(
    root: &Path,
    binding: &Map<String, Value>,
    metadata: &Map<String, Value>,
    kind: &str,
    source: &str,
    total: &mut usize,
) -> Option<()> {
    let root = root.canonicalize().ok()?;
    let path = root.join(file_path(binding)?).canonicalize().ok()?;
    if !path.starts_with(&root) {
        return None;
    }
    let before_open = std::fs::metadata(&path).ok()?;
    if !before_open.is_file() || before_open.len() > MAX_FILE_BYTES {
        return None;
    }
    let file = File::open(path).ok()?;
    let stat = file.metadata().ok()?;
    if !stat.is_file() || stat.len() > MAX_FILE_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes).ok()?;
    *total = total.checked_add(bytes.len())?;
    if bytes.len() > MAX_FILE_BYTES as usize || *total > MAX_TOTAL_BYTES {
        return None;
    }
    let window_start = match metadata.get(&format!("{kind}_start")) {
        Some(value) => Some(value.as_u64()?),
        None => None,
    };
    let range = stated_range(metadata)?.or(stated_range(binding)?);
    if let Some(start) = window_start {
        let start = line_offset(&bytes, start)?;
        return bytes
            .get(start..start.checked_add(source.len())?)
            .filter(|bytes| *bytes == source.as_bytes())
            .map(|_| ());
    }
    if kind == "content" {
        let line = metadata.get("line")?.as_u64()?;
        let start = line_offset(&bytes, line)?;
        let tail = bytes.get(start..)?;
        let end = tail
            .iter()
            .position(|byte| *byte == b'\n')
            .unwrap_or(tail.len());
        return (tail.get(..end)? == source.as_bytes()).then_some(());
    }
    let expected = match range {
        Some((start, end)) => {
            if end < start {
                return None;
            }
            let start = line_offset(&bytes, start)?;
            let end = match line_offset(&bytes, end.checked_add(1)?) {
                Some(offset) => offset,
                None if !bytes.ends_with(b"\n")
                    && end == bytes.split(|byte| *byte == b'\n').count() as u64 =>
                {
                    bytes.len()
                }
                None => return None,
            };
            bytes.get(start..end)?
        }
        None => bytes.as_slice(),
    };
    (expected == source.as_bytes()).then_some(())
}

fn stated_range(object: &Map<String, Value>) -> Option<Option<(u64, u64)>> {
    let explicit = match (object.get("start_line"), object.get("end_line")) {
        (Some(start), Some(end)) => Some(Some((start.as_u64()?, end.as_u64()?))),
        (None, None) => Some(None),
        _ => None,
    }?;
    let encoded = if let Some(lines) = object.get("lines") {
        let (start, end) = lines.as_str()?.split_once('-')?;
        Some((start.parse().ok()?, end.parse().ok()?))
    } else {
        None
    };
    if explicit.zip(encoded).is_some_and(|(a, b)| a != b) {
        return None;
    }
    let range = explicit.or(encoded);
    if range.is_some_and(|(start, end)| start == 0 || end < start) {
        return None;
    }
    Some(range)
}

fn line_offset(bytes: &[u8], line: u64) -> Option<usize> {
    if line == 0 {
        return None;
    }
    if line == 1 {
        return Some(0);
    }
    let mut current = 1;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            current += 1;
            if current == line {
                return Some(index + 1);
            }
        }
    }
    None
}

#[cfg(test)]
#[path = "tests/source_guard.rs"]
mod tests;
