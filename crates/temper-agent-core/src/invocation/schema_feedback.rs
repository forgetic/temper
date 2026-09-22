//! Model guidance derived only from the finalized registry schema.
//!
//! This is a bounded explanation, never an alternative acceptance check. It
//! cannot retain argument values, unknown object keys, or provider tool names.

use serde_json::Value;

#[derive(Clone, Debug)]
pub(crate) struct SchemaFeedback(String);

impl SchemaFeedback {
    pub(super) fn from_schema(name: &str, schema: &Value, arguments: &Value) -> Option<Self> {
        if name.len() > 128
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
        {
            return None;
        }
        let detail = explain(schema, arguments, "$", 0, &mut 128)
            .unwrap_or_else(|| "use the required fields and types in the tool schema".to_string());
        Some(Self(format!("Tool {name}: {detail}.")))
    }

    pub(crate) fn message(&self) -> &str {
        &self.0
    }
}

fn field_path(path: &str, key: &str) -> Option<String> {
    if key.is_empty()
        || key.len() > 64
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return None;
    }
    let path = format!("{path}.{key}");
    (path.len() <= 256).then_some(path)
}

fn explain(
    schema: &Value,
    value: &Value,
    path: &str,
    depth: usize,
    remaining: &mut usize,
) -> Option<String> {
    if depth >= 8 || *remaining == 0 {
        return None;
    }
    *remaining -= 1;
    let expected = schema.get("type").and_then(Value::as_str);
    let matches = match expected {
        Some("object") => value.is_object(),
        Some("array") => value.is_array(),
        Some("string") => value.is_string(),
        Some("integer") => value.is_i64() || value.is_u64(),
        Some("number") => value.is_number(),
        Some("boolean") => value.is_boolean(),
        Some("null") => value.is_null(),
        _ => true,
    };
    if !matches {
        return Some(format!("{path} must have type {}", expected?));
    }
    if let Some(detail) = explain_object(schema, value, path, depth, remaining) {
        return Some(detail);
    }
    if let (Some(items), Some(values)) = (schema.get("items"), value.as_array()) {
        let item_path = format!("{path}[]");
        if item_path.len() <= 256 {
            for value in values.iter().take(64) {
                if let Some(detail) = explain(items, value, &item_path, depth + 1, remaining) {
                    return Some(detail);
                }
            }
        }
    }
    // Every allOf branch is mandatory. Alternative branches are deliberately
    // not explained: an individual branch's requirement might not apply.
    for branch in schema
        .get("allOf")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(8)
    {
        if let Some(detail) = explain(branch, value, path, depth + 1, remaining) {
            return Some(detail);
        }
    }
    None
}

fn explain_object(
    schema: &Value,
    value: &Value,
    path: &str,
    depth: usize,
    remaining: &mut usize,
) -> Option<String> {
    let object = value.as_object()?;
    let properties = schema.get("properties")?.as_object()?;
    for key in schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(64)
    {
        let Some((key, _)) = key.as_str().and_then(|key| properties.get_key_value(key)) else {
            continue;
        };
        if !object.contains_key(key) {
            if let Some(path) = field_path(path, key) {
                return Some(format!("required field {path} is missing"));
            }
        }
    }
    // Iterate schema-owned keys, never keys copied from the supplied object.
    for (key, property) in properties.iter().take(64) {
        if let (Some(value), Some(path)) = (object.get(key), field_path(path, key)) {
            if let Some(detail) = explain(property, value, &path, depth + 1, remaining) {
                return Some(detail);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests;
