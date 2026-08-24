//! Ephemeral provider validator for the mapped exact source-selection scenario.
//!
//! Provider arguments, selectors, roots, source, and payloads remain temporary.
//! Only closed tool order and checkpoint categories enter retained evidence.

use std::fs;

use serde_json::Value as JsonValue;

use super::stable_rebind::{confirmed_project_from_calls, validate_stable_rebind_contract};
use super::{FakeMcpServer, McpToolCallEvidence};

pub(super) fn validate(mcp: &FakeMcpServer, calls: &[McpToolCallEvidence]) -> Result<(), String> {
    let expected_tools = [
        "index_status",
        "index_repository",
        "index_status",
        "search_code",
        "get_code_snippet",
        "trace_path",
        "get_code_snippet",
        "search_code",
        "search_code",
        "search_code",
        "trace_path",
        "search_graph",
        "get_code_snippet",
    ];
    if calls
        .iter()
        .map(|call| call.name.as_str())
        .collect::<Vec<_>>()
        != expected_tools
        || calls.iter().any(|call| call.is_error)
    {
        return Err(
            "exact selection fixture requires ten ordered successful graph reads and no malformed provider invocation"
                .into(),
        );
    }
    if calls
        .iter()
        .any(|call| call.arguments.get("decision_evidence_kind").is_some())
    {
        return Err("wrapper-owned decision evidence reached the MCP provider".into());
    }

    let requested = calls[1]
        .arguments
        .get("name")
        .and_then(JsonValue::as_str)
        .filter(|name| name.starts_with("temper-v1-"))
        .ok_or("exact selection fixture did not use a stable provider identity")?;
    let confirmed = confirmed_project_from_calls(calls, requested)?;
    if calls[3..].iter().any(|call| {
        call.arguments.get("project").and_then(JsonValue::as_str) != Some(confirmed.as_str())
    }) {
        return Err("exact selection fixture lost its confirmed current-root binding".into());
    }

    let tokens = selection_tokens(mcp)?;
    let implementation = token(&tokens, "implementation")?;
    let caller = token(&tokens, "caller")?;
    let focused_test = token(&tokens, "focused_test")?;
    let expected_arguments = [
        (3, "pattern", "route affinity"),
        (4, "qualified_name", implementation),
        (5, "function_name", implementation),
        (6, "qualified_name", caller),
        (7, "pattern", implementation),
        (8, "pattern", implementation),
        (9, "pattern", implementation),
        (10, "function_name", caller),
        (11, "query", "focused alias retry behavior"),
        (12, "qualified_name", focused_test),
    ];
    if expected_arguments.iter().any(|(index, field, expected)| {
        calls[*index]
            .arguments
            .get(*field)
            .and_then(JsonValue::as_str)
            != Some(*expected)
    }) {
        return Err("exact selection fixture did not consume its returned selectors".into());
    }
    if calls[10].arguments.get("mode").and_then(JsonValue::as_str) != Some("calls")
        || calls[10]
            .arguments
            .get("direction")
            .and_then(JsonValue::as_str)
            != Some("inbound")
        || calls[10]
            .arguments
            .get("include_tests")
            .and_then(JsonValue::as_bool)
            != Some(true)
    {
        return Err("exact selection fixture omitted the focused-test forest traversal".into());
    }

    let expected_events = [
        "served_selection_root",
        "served_selection_implementation_source",
        "served_selection_caller_trace",
        "served_selection_caller_source",
        "served_selection_generic",
        "served_selection_generic",
        "served_selection_generic",
        "served_selection_forest",
        "served_selection_focused_fallback",
        "served_selection_focused_source",
    ];
    if calls[3..]
        .iter()
        .map(|call| call.fixture_event.as_deref().unwrap_or_default())
        .collect::<Vec<_>>()
        != expected_events
    {
        return Err("exact selection fixture omitted a closed interleaved checkpoint".into());
    }

    validate_stable_rebind_contract(mcp, calls, requested)
}

fn selection_tokens(mcp: &FakeMcpServer) -> Result<serde_json::Map<String, JsonValue>, String> {
    let raw = fs::read_to_string(&mcp.state_path)
        .map_err(|_| "exact selection fixture state was unavailable".to_string())?;
    let state: JsonValue = serde_json::from_str(&raw)
        .map_err(|_| "exact selection fixture state was malformed".to_string())?;
    state
        .get("focused_relevance_tokens")
        .and_then(JsonValue::as_object)
        .cloned()
        .ok_or("exact selection fixture omitted transient selectors".to_string())
}

fn token<'a>(
    tokens: &'a serde_json::Map<String, JsonValue>,
    name: &str,
) -> Result<&'a str, String> {
    tokens
        .get(name)
        .and_then(JsonValue::as_str)
        .ok_or("exact selection fixture omitted a transient selector".to_string())
}
