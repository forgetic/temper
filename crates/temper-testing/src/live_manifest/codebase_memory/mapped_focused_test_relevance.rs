//! Ephemeral provider validator for focused-test fallback source relevance.
//!
//! Exact selectors, source, roots, and provider payloads stay in temporary
//! state. Only closed ordering and lineage checkpoint categories are retained.

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
            "focused-test relevance fixture requires one ordered successful fallback chain and no mismatched provider invocation"
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
        .ok_or("focused-test relevance fixture did not use a stable provider identity")?;
    let confirmed = confirmed_project_from_calls(calls, requested)?;
    if calls[3..].iter().any(|call| {
        call.arguments.get("project").and_then(JsonValue::as_str) != Some(confirmed.as_str())
    }) {
        return Err("focused-test fallback lost its confirmed current-root binding".into());
    }

    let tokens = relevance_tokens(mcp)?;
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
        (9, "function_name", caller),
        (10, "query", "focused alias retry behavior"),
        (11, "qualified_name", focused_test),
    ];
    if expected_arguments.iter().any(|(index, field, expected)| {
        calls[*index]
            .arguments
            .get(*field)
            .and_then(JsonValue::as_str)
            != Some(*expected)
    }) {
        return Err("focused-test relevance fixture did not consume its returned selectors".into());
    }
    if calls[9].arguments.get("mode").and_then(JsonValue::as_str) != Some("calls")
        || calls[9]
            .arguments
            .get("direction")
            .and_then(JsonValue::as_str)
            != Some("inbound")
        || calls[9]
            .arguments
            .get("include_tests")
            .and_then(JsonValue::as_bool)
            != Some(true)
    {
        return Err("focused-test relevance fixture omitted the exact empty traversal".into());
    }

    let expected_events = [
        "served_focus_root",
        "served_focus_implementation_source",
        "served_focus_caller_trace",
        "served_focus_caller_source",
        "served_focus_non_progress",
        "served_focus_non_progress",
        "served_focus_empty_traversal",
        "served_focus_fallback",
        "served_focus_test_source",
    ];
    if calls[3..]
        .iter()
        .map(|call| call.fixture_event.as_deref().unwrap_or_default())
        .collect::<Vec<_>>()
        != expected_events
    {
        return Err("focused-test relevance fixture omitted a closed lineage checkpoint".into());
    }

    validate_stable_rebind_contract(mcp, calls, requested)
}

fn relevance_tokens(mcp: &FakeMcpServer) -> Result<serde_json::Map<String, JsonValue>, String> {
    let raw = fs::read_to_string(&mcp.state_path)
        .map_err(|_| "focused-test relevance fixture state was unavailable".to_string())?;
    let state: JsonValue = serde_json::from_str(&raw)
        .map_err(|_| "focused-test relevance fixture state was malformed".to_string())?;
    state
        .get("focused_relevance_tokens")
        .and_then(JsonValue::as_object)
        .cloned()
        .ok_or("focused-test relevance fixture omitted transient selectors".to_string())
}

fn token<'a>(
    tokens: &'a serde_json::Map<String, JsonValue>,
    name: &str,
) -> Result<&'a str, String> {
    tokens
        .get(name)
        .and_then(JsonValue::as_str)
        .ok_or("focused-test relevance fixture omitted a transient selector".to_string())
}
