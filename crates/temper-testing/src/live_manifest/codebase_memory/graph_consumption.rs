use serde_json::Value as JsonValue;

use super::stable_rebind::{confirmed_project_from_calls, validate_stable_rebind_contract};
use super::{FakeMcpServer, McpToolCallEvidence};

/// Validate the narrow, production-shaped graph chain without retaining MCP
/// arguments or source in durable evidence. The fixture log is ephemeral; only
/// safe names and aggregate counts cross the live-run evidence boundary.
pub(super) fn validate(mcp: &FakeMcpServer, calls: &[McpToolCallEvidence]) -> Result<(), String> {
    let expected_tools = [
        "index_status",
        "index_repository",
        "index_status",
        "search_graph",
        "search_code",
        "trace_path",
        "get_code_snippet",
        "get_code_snippet",
    ];
    let actual_tools = calls
        .iter()
        .map(|call| call.name.as_str())
        .collect::<Vec<_>>();
    if actual_tools != expected_tools {
        return Err(format!(
            "graph-consumption fixture requires only its declared ordered MCP chain; expected {expected_tools:?}, got {actual_tools:?}"
        ));
    }
    if calls.iter().any(|call| call.is_error) {
        return Err(
            "graph-consumption fixture recorded an unsuccessful declared MCP call".to_string(),
        );
    }
    let requested = calls[1]
        .arguments
        .get("name")
        .and_then(JsonValue::as_str)
        .filter(|name| name.starts_with("temper-v1-"))
        .ok_or("graph-consumption upsert did not use a stable provider identity")?;
    if calls[0]
        .arguments
        .get("project")
        .and_then(JsonValue::as_str)
        != Some(requested)
        || calls[0].fixture_event.as_deref() != Some("fresh_prior_binding")
    {
        return Err(
            "graph-consumption initial discovery was not a targeted stable-key lookup".to_string(),
        );
    }
    let confirmed = confirmed_project_from_calls(calls, requested)?;
    for call in &calls[3..] {
        if call.arguments.get("project").and_then(JsonValue::as_str) != Some(confirmed.as_str()) {
            return Err(format!(
                "graph-consumption call `{}` did not use the confirmed current-root provider identity",
                call.name
            ));
        }
    }
    let expected_targets = [
        (3, "query", "alias retry worker affinity"),
        (4, "pattern", "retry_worker_topic"),
        (5, "function_name", "retry_worker_topic"),
        (6, "qualified_name", "retry_worker_topic"),
        (7, "qualified_name", "retry_worker_topic_retry_affinity"),
    ];
    for (index, field, expected) in expected_targets {
        if calls[index]
            .arguments
            .get(field)
            .and_then(JsonValue::as_str)
            != Some(expected)
        {
            return Err(format!(
                "graph-consumption call `{}` did not use declared targeted {field}",
                calls[index].name
            ));
        }
    }
    if calls[4].fixture_event.as_deref() != Some("served_current_root_code_refinement")
        || calls[5].fixture_event.as_deref() != Some("served_current_root_graph_trace")
        || calls[6].fixture_event.as_deref() != Some("served_current_root_source")
        || calls[7].fixture_event.as_deref() != Some("served_current_root_source")
    {
        return Err(
            "graph-consumption fixture did not serve refinement, trace, and both source reads from the confirmed current root"
                .to_string(),
        );
    }
    validate_stable_rebind_contract(mcp, calls, requested)
}
