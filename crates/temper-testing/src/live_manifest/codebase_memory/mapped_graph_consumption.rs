//! Ephemeral validator for feature #1009's approved multi-part transcript.
//!
//! Provider symbols and paths remain in the temporary MCP state and call log.
//! The checked-in and returned evidence contains only closed checkpoint names.
//!
//! The accepted call list includes one unavailable descendant after the five
//! successful lineage calls. The runtime Jig proves that it falls back to a
//! conventional read without making another graph request.

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
        "search_code",
        "trace_path",
        "get_code_snippet",
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
            "mapped graph fixture requires only its declared ordered multi-call chain".into(),
        );
    }
    if calls[..8].iter().any(|call| call.is_error) || !calls[8].is_error {
        return Err(
            "mapped graph fixture requires five successful lineage calls followed by one expected unavailable descendant"
                .into(),
        );
    }

    let requested = calls[1]
        .arguments
        .get("name")
        .and_then(JsonValue::as_str)
        .filter(|name| name.starts_with("temper-v1-"))
        .ok_or("mapped graph fixture did not use a stable provider identity")?;
    let confirmed = confirmed_project_from_calls(calls, requested)?;
    if calls[3..].iter().any(|call| {
        call.arguments.get("project").and_then(JsonValue::as_str) != Some(confirmed.as_str())
    }) {
        return Err(
            "mapped graph fixture did not retain the confirmed current-root binding".into(),
        );
    }

    let tokens = mapped_tokens(mcp)?;
    let implementation = token(&tokens, "implementation")?;
    let caller = token(&tokens, "caller")?;
    let source = token(&tokens, "source")?;
    let unavailable = token(&tokens, "unavailable")?;
    let short = terminal_name(implementation)?;
    if calls[4]
        .arguments
        .get("pattern")
        .and_then(JsonValue::as_str)
        != Some(short)
        || calls[5]
            .arguments
            .get("function_name")
            .and_then(JsonValue::as_str)
            != Some(short)
        || calls[6]
            .arguments
            .get("qualified_name")
            .and_then(JsonValue::as_str)
            != Some(caller)
        || calls[7]
            .arguments
            .get("qualified_name")
            .and_then(JsonValue::as_str)
            != Some(source)
        || calls[8]
            .arguments
            .get("qualified_name")
            .and_then(JsonValue::as_str)
            != Some(unavailable)
    {
        return Err("mapped graph fixture did not consume the approved transformed chain".into());
    }
    let closure_profile =
        mcp.lifecycle_profile.as_deref() == Some("mapped-live-ordinary-tool-convergence");
    let expected_final_event = if closure_profile {
        "served_graph_closure"
    } else {
        "served_mapped_unavailable"
    };
    let expected_events = [
        "served_mapped_root",
        "served_mapped_carry_forward",
        "served_mapped_carry_forward",
        "served_mapped_current_root_source",
        "served_mapped_current_root_source",
        expected_final_event,
    ];
    if calls[3..]
        .iter()
        .map(|call| call.fixture_event.as_deref().unwrap_or_default())
        .collect::<Vec<_>>()
        != expected_events
    {
        return Err("mapped graph fixture omitted a privacy-safe lineage checkpoint".into());
    }
    validate_stable_rebind_contract(mcp, calls, requested)
}

fn mapped_tokens(mcp: &FakeMcpServer) -> Result<serde_json::Map<String, JsonValue>, String> {
    let raw = fs::read_to_string(&mcp.state_path)
        .map_err(|_| "mapped graph fixture state was unavailable".to_string())?;
    let state: JsonValue = serde_json::from_str(&raw)
        .map_err(|_| "mapped graph fixture state was malformed".to_string())?;
    state
        .get("mapped_graph_tokens")
        .and_then(JsonValue::as_object)
        .cloned()
        .ok_or("mapped graph fixture omitted its transient selections".to_string())
}

fn token<'a>(
    tokens: &'a serde_json::Map<String, JsonValue>,
    name: &str,
) -> Result<&'a str, String> {
    tokens
        .get(name)
        .and_then(JsonValue::as_str)
        .ok_or("mapped graph fixture omitted a transient selection".to_string())
}

fn terminal_name(qualified: &str) -> Result<&str, String> {
    qualified
        .rsplit_once("::")
        .map(|(_, terminal)| terminal)
        .filter(|terminal| !terminal.is_empty())
        .ok_or("mapped graph fixture selection was not transformable".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, arguments: JsonValue, event: &str) -> McpToolCallEvidence {
        McpToolCallEvidence {
            name: name.to_string(),
            arguments,
            delay_ms: (name == "index_repository").then_some(750),
            is_error: false,
            fixture_event: Some(event.to_string()),
        }
    }

    fn fixture() -> (tempfile::TempDir, FakeMcpServer, Vec<McpToolCallEvidence>) {
        let workspace = tempfile::tempdir().expect("workspace");
        let requested = "temper-v1-scenario";
        let confirmed = "normalized-temper-v1-scenario";
        let implementation = "crate::fixture::routing::worker_slot";
        let caller = "crate::fixture::delivery::DeliveryAttempt";
        let source = "crate::fixture::delivery::worker_for";
        let unavailable = "crate::fixture::unavailable";
        let state_path = workspace.path().join("mcp.state.json");
        fs::write(
            &state_path,
            serde_json::json!({
                "projects": {confirmed: {
                    "requested_stable_project": requested,
                    "repo_path": "/workspace/demo",
                    "binding": "current_prepared_checkout"
                }},
                "counters": {"project_creations": 1, "rebinds": 1},
                "mapped_graph_tokens": {
                    "implementation": implementation,
                    "caller": caller,
                    "source": source,
                    "unavailable": unavailable
                }
            })
            .to_string(),
        )
        .expect("state");
        let mcp = FakeMcpServer {
            script_path: workspace.path().join("fake.py"),
            log_path: workspace.path().join("mcp.jsonl"),
            state_path,
            project: "demo".into(),
            lifecycle_profile: Some("mapped-live-graph-consumption".into()),
            safe_tools: vec!["search_graph".into()],
            hidden_tools: vec!["index_repository".into()],
            readiness_delay_ms: 750,
            forced_systemic_failure: None,
        };
        let calls = vec![
            call(
                "index_status",
                serde_json::json!({"project": requested}),
                "fresh_prior_binding",
            ),
            call(
                "index_repository",
                serde_json::json!({"name": requested, "repo_path": "/workspace/demo"}),
                "normalized_current_root_upsert",
            ),
            call(
                "index_status",
                serde_json::json!({"project": confirmed}),
                "current_root_confirmed",
            ),
            call(
                "search_graph",
                serde_json::json!({"project": confirmed}),
                "served_mapped_root",
            ),
            call(
                "search_code",
                serde_json::json!({"project": confirmed, "pattern": "worker_slot"}),
                "served_mapped_carry_forward",
            ),
            call(
                "trace_path",
                serde_json::json!({"project": confirmed, "function_name": "worker_slot"}),
                "served_mapped_carry_forward",
            ),
            call(
                "get_code_snippet",
                serde_json::json!({"project": confirmed, "qualified_name": caller}),
                "served_mapped_current_root_source",
            ),
            call(
                "get_code_snippet",
                serde_json::json!({"project": confirmed, "qualified_name": source}),
                "served_mapped_current_root_source",
            ),
            McpToolCallEvidence {
                name: "get_code_snippet".to_string(),
                arguments: serde_json::json!({"project": confirmed, "qualified_name": unavailable}),
                delay_ms: None,
                is_error: true,
                fixture_event: Some("served_mapped_unavailable".to_string()),
            },
        ];
        (workspace, mcp, calls)
    }

    #[test]
    fn accepts_only_the_complete_mapped_chain() {
        let (_workspace, mcp, calls) = fixture();
        validate(&mcp, &calls).expect("valid mapped chain");
    }

    #[test]
    fn rejects_denied_or_incomplete_mapped_chains() {
        let (_workspace, mcp, mut calls) = fixture();
        calls[5].arguments["function_name"] = JsonValue::String("unrelated".into());
        assert!(
            validate(&mcp, &calls)
                .unwrap_err()
                .contains("approved transformed chain")
        );

        let (_workspace, mcp, mut calls) = fixture();
        calls.pop();
        assert!(
            validate(&mcp, &calls)
                .unwrap_err()
                .contains("ordered multi-call chain")
        );

        let (_workspace, mcp, mut calls) = fixture();
        calls[6].arguments["project"] = JsonValue::String("other-root".into());
        assert!(
            validate(&mcp, &calls)
                .unwrap_err()
                .contains("current-root binding")
        );
    }
}
