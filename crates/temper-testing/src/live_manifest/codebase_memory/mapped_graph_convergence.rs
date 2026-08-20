//! Ephemeral validator for feature #1026's progress-bounded graph scenario.
//!
//! Provider values and source stay in temporary state. This validator retains
//! only the closed provider-call order and fixture checkpoint categories, and
//! proves that post-convergence model attempts never reached MCP.

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
        "trace_path",
        "get_code_snippet",
        "search_graph",
        "search_graph",
        "search_code",
        "trace_path",
        "search_code",
        "get_code_snippet",
        "get_code_snippet",
    ];
    if calls
        .iter()
        .map(|call| call.name.as_str())
        .collect::<Vec<_>>()
        != expected_tools
    {
        return Err(
            "graph-convergence fixture requires its declared provider chain and no post-convergence provider invocation"
                .into(),
        );
    }
    if calls
        .iter()
        .enumerate()
        .any(|(index, call)| call.is_error != (index == 5))
    {
        return Err(
            "graph-convergence fixture requires one unavailable pre-completion source and nine successful graph results"
                .into(),
        );
    }

    let requested = calls[1]
        .arguments
        .get("name")
        .and_then(JsonValue::as_str)
        .filter(|name| name.starts_with("temper-v1-"))
        .ok_or("graph-convergence fixture did not use a stable provider identity")?;
    let confirmed = confirmed_project_from_calls(calls, requested)?;
    if calls[3..].iter().any(|call| {
        call.arguments.get("project").and_then(JsonValue::as_str) != Some(confirmed.as_str())
    }) {
        return Err("graph-convergence fixture lost its confirmed current-root binding".into());
    }

    let tokens = convergence_tokens(mcp)?;
    let preflight = token(&tokens, "preflight")?;
    let unavailable = token(&tokens, "unavailable")?;
    let implementation = token(&tokens, "implementation")?;
    let caller = token(&tokens, "caller")?;
    let behavioral_test = token(&tokens, "behavioral_test")?;
    let implementation_short = terminal_name(implementation)?;

    let expected_arguments = [
        (3, "query", "availability preflight"),
        (4, "function_name", terminal_name(preflight)?),
        (5, "qualified_name", unavailable),
        (6, "query", "routing implementation affinity"),
        (7, "query", "focused alias retry behavior"),
        (8, "pattern", implementation_short),
        (9, "function_name", implementation_short),
        (10, "pattern", implementation_short),
        (11, "qualified_name", caller),
        (12, "qualified_name", behavioral_test),
    ];
    if expected_arguments.iter().any(|(index, field, expected)| {
        calls[*index]
            .arguments
            .get(*field)
            .and_then(JsonValue::as_str)
            != Some(*expected)
    }) {
        return Err(
            "graph-convergence fixture did not consume its transient provider selections".into(),
        );
    }

    let expected_events = [
        "served_convergence_preflight_root",
        "served_convergence_preflight_trace",
        "served_convergence_unavailable",
        "served_convergence_root",
        "served_convergence_root",
        "served_convergence_refinement",
        "served_convergence_trace",
        "served_convergence_duplicate",
        "served_convergence_source",
        "served_convergence_source",
    ];
    if calls[3..]
        .iter()
        .map(|call| call.fixture_event.as_deref().unwrap_or_default())
        .collect::<Vec<_>>()
        != expected_events
    {
        return Err("graph-convergence fixture omitted a closed aggregate checkpoint".into());
    }
    validate_stable_rebind_contract(mcp, calls, requested)
}

fn convergence_tokens(mcp: &FakeMcpServer) -> Result<serde_json::Map<String, JsonValue>, String> {
    let raw = fs::read_to_string(&mcp.state_path)
        .map_err(|_| "graph-convergence fixture state was unavailable".to_string())?;
    let state: JsonValue = serde_json::from_str(&raw)
        .map_err(|_| "graph-convergence fixture state was malformed".to_string())?;
    state
        .get("graph_convergence_tokens")
        .and_then(JsonValue::as_object)
        .cloned()
        .ok_or("graph-convergence fixture omitted transient selections".to_string())
}

fn token<'a>(
    tokens: &'a serde_json::Map<String, JsonValue>,
    name: &str,
) -> Result<&'a str, String> {
    tokens
        .get(name)
        .and_then(JsonValue::as_str)
        .ok_or("graph-convergence fixture omitted a transient selection".to_string())
}

fn terminal_name(qualified: &str) -> Result<&str, String> {
    qualified
        .rsplit_once("::")
        .map(|(_, terminal)| terminal)
        .filter(|terminal| !terminal.is_empty())
        .ok_or("graph-convergence fixture selection was not transformable".to_string())
}
