//! Ephemeral provider validator for mapped post-correction authorization.
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
        "search_graph",
        "search_graph",
        "get_code_snippet",
        "get_code_snippet",
        "get_code_snippet",
        "trace_path",
        "get_code_snippet",
        "get_code_snippet",
        "get_code_snippet",
        "get_code_snippet",
        "get_code_snippet",
    ];
    if calls
        .iter()
        .map(|call| call.name.as_str())
        .collect::<Vec<_>>()
        != expected_tools
        || calls
            .iter()
            .enumerate()
            .any(|(index, call)| call.is_error != (index == 6))
    {
        return Err(
            "post-correction authorization requires ten ordered successful graph results and one controlled unavailable-source rejection"
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
        .ok_or("post-correction authorization did not use a stable provider identity")?;
    let confirmed = confirmed_project_from_calls(calls, requested)?;
    if calls[3..].iter().any(|call| {
        call.arguments.get("project").and_then(JsonValue::as_str) != Some(confirmed.as_str())
    }) {
        return Err("post-correction authorization lost its confirmed current-root binding".into());
    }

    let tokens = convergence_tokens(mcp)?;
    let model = token(&tokens, "model")?;
    let route = token(&tokens, "implementation")?;
    let caller = token(&tokens, "caller")?;
    let focused_test = token(&tokens, "behavioral_test")?;
    let root_queries = calls[3..5]
        .iter()
        .filter_map(|call| call.arguments.get("query").and_then(JsonValue::as_str))
        .collect::<std::collections::BTreeSet<_>>();
    if root_queries
        != std::collections::BTreeSet::from([
            "focused routing regression",
            "routing implementation authority",
        ])
    {
        return Err("post-correction authorization omitted an independent root".into());
    }
    let expected_arguments = [
        (5, "qualified_name", model),
        (7, "qualified_name", model),
        (8, "function_name", terminal_name(model)?),
        (9, "qualified_name", caller),
        (10, "qualified_name", focused_test),
        (13, "qualified_name", route),
    ];
    if expected_arguments.iter().any(|(index, field, expected)| {
        calls[*index]
            .arguments
            .get(*field)
            .and_then(JsonValue::as_str)
            != Some(*expected)
    }) {
        return Err(
            "post-correction authorization did not consume its presented opaque selectors".into(),
        );
    }
    if calls[8].arguments.get("mode").and_then(JsonValue::as_str) != Some("calls")
        || calls[8]
            .arguments
            .get("direction")
            .and_then(JsonValue::as_str)
            != Some("inbound")
        || calls[8]
            .arguments
            .get("include_tests")
            .and_then(JsonValue::as_bool)
            != Some(false)
    {
        return Err(
            "post-correction authorization omitted the bounded implementation trace".into(),
        );
    }
    let correction_previews = calls[11..13]
        .iter()
        .filter_map(|call| {
            call.arguments
                .get("qualified_name")
                .and_then(JsonValue::as_str)
        })
        .collect::<std::collections::BTreeSet<_>>();
    if correction_previews != std::collections::BTreeSet::from([caller, route]) {
        return Err(
            "post-correction authorization did not inspect both presented corrections".into(),
        );
    }

    let expected_events = [
        "served_selection_root",
        "served_selection_root",
        "served_selection_provisional_preview",
        "",
        "served_selection_provisional_source",
        "served_selection_correction_trace",
        "served_selection_caller_source",
        "served_selection_focused_source",
        "served_selection_correction_preview",
        "served_selection_correction_preview",
        "served_selection_corrected_source",
    ];
    if calls[3..]
        .iter()
        .map(|call| call.fixture_event.as_deref().unwrap_or_default())
        .collect::<Vec<_>>()
        != expected_events
    {
        return Err("post-correction authorization omitted a closed transition checkpoint".into());
    }

    validate_stable_rebind_contract(mcp, calls, requested)
}

fn convergence_tokens(mcp: &FakeMcpServer) -> Result<serde_json::Map<String, JsonValue>, String> {
    let raw = fs::read_to_string(&mcp.state_path)
        .map_err(|_| "post-correction authorization state was unavailable".to_string())?;
    let state: JsonValue = serde_json::from_str(&raw)
        .map_err(|_| "post-correction authorization state was malformed".to_string())?;
    state
        .get("graph_convergence_tokens")
        .and_then(JsonValue::as_object)
        .cloned()
        .ok_or("post-correction authorization omitted transient selectors".to_string())
}

fn token<'a>(
    tokens: &'a serde_json::Map<String, JsonValue>,
    name: &str,
) -> Result<&'a str, String> {
    tokens
        .get(name)
        .and_then(JsonValue::as_str)
        .ok_or("post-correction authorization omitted a transient selector".to_string())
}

fn terminal_name(qualified: &str) -> Result<&str, String> {
    qualified
        .rsplit_once("::")
        .map(|(_, terminal)| terminal)
        .filter(|terminal| !terminal.is_empty())
        .ok_or("post-correction authorization selector was not transformable".to_string())
}
