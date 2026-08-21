//! Shell-owned classification of ordinary tool failures.

use crate::machine::{ToolFailureDiagnostic, ToolFailureReason};

pub(super) fn trusted_first_party_failure(
    name: &str,
    output: &tongs::tools::ToolOutput,
) -> Option<ToolFailureDiagnostic> {
    if !matches!(name, "forge_get_item" | "forge_list_related") {
        return None;
    }
    let code = output.details.as_ref()?.get("code")?.as_str()?;
    match code {
        "invalid_request" => Some(ToolFailureDiagnostic::schema(
            ToolFailureReason::InvalidArguments,
        )),
        "not_authorized" | "limit_exceeded" => Some(ToolFailureDiagnostic::access_denied()),
        "not_found" | "forge_unavailable" => Some(ToolFailureDiagnostic::execution(
            ToolFailureReason::ToolReportedFailure,
        )),
        _ => None,
    }
}

pub(super) fn advertised_arguments_match(
    schema: &serde_json::Value,
    value: &serde_json::Value,
) -> bool {
    if schema
        .get("enum")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|values| !values.contains(value))
    {
        return false;
    }
    match schema.get("type").and_then(serde_json::Value::as_str) {
        Some("object") => {
            let Some(object) = value.as_object() else {
                return false;
            };
            if schema
                .get("required")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|required| {
                    required
                        .iter()
                        .any(|key| key.as_str().is_none_or(|key| !object.contains_key(key)))
                })
            {
                return false;
            }
            let properties = schema
                .get("properties")
                .and_then(serde_json::Value::as_object);
            object.iter().all(|(key, value)| {
                properties
                    .and_then(|properties| properties.get(key))
                    .is_none_or(|schema| advertised_arguments_match(schema, value))
            })
        }
        Some("array") => value.as_array().is_some_and(|values| {
            schema.get("items").is_none_or(|item_schema| {
                values
                    .iter()
                    .all(|value| advertised_arguments_match(item_schema, value))
            })
        }),
        Some("string") => value.is_string(),
        Some("integer") => value.as_i64().is_some() || value.as_u64().is_some(),
        Some("number") => value.is_number(),
        Some("boolean") => value.is_boolean(),
        Some("null") => value.is_null(),
        Some(_) => false,
        None => true,
    }
}
