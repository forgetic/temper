//! Deterministic privacy-safe fixture checks for typed decision-anchor lineage.
//!
//! Provider-shaped values remain only in the temporary MCP state. This
//! validator sees those values solely to prove the accepted call chain used an
//! approved qualified-symbol-to-terminal-function transformation.

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
        "get_code_snippet",
    ];
    if calls
        .iter()
        .map(|call| call.name.as_str())
        .collect::<Vec<_>>()
        != expected_tools
    {
        return Err(
            "typed lineage fixture requires only its declared ordered MCP chain".to_string(),
        );
    }
    if calls.iter().any(|call| call.is_error) {
        return Err("typed lineage fixture recorded an unsuccessful declared MCP call".to_string());
    }

    let requested = calls[1]
        .arguments
        .get("name")
        .and_then(JsonValue::as_str)
        .filter(|name| name.starts_with("temper-v1-"))
        .ok_or("typed lineage fixture did not use a stable provider identity")?;
    if calls[0]
        .arguments
        .get("project")
        .and_then(JsonValue::as_str)
        != Some(requested)
        || calls[0].fixture_event.as_deref() != Some("fresh_prior_binding")
    {
        return Err(
            "typed lineage fixture did not start from a targeted current-root lookup".to_string(),
        );
    }
    let confirmed = confirmed_project_from_calls(calls, requested)?;
    if calls[3..].iter().any(|call| {
        call.arguments.get("project").and_then(JsonValue::as_str) != Some(confirmed.as_str())
    }) {
        return Err(
            "typed lineage fixture did not retain the confirmed current-root binding".to_string(),
        );
    }

    let tokens = typed_tokens(mcp)?;
    let root = token(&tokens, "root")?;
    let behavioral_test = token(&tokens, "behavioral_test")?;
    let transformed = terminal_function_name(root)?;
    if transformed == root
        || calls[4]
            .arguments
            .get("function_name")
            .and_then(JsonValue::as_str)
            != Some(transformed)
        || calls[5]
            .arguments
            .get("qualified_name")
            .and_then(JsonValue::as_str)
            != Some(root)
        || calls[6]
            .arguments
            .get("qualified_name")
            .and_then(JsonValue::as_str)
            != Some(behavioral_test)
    {
        return Err(
            "typed lineage fixture did not use its approved transformed descendant chain"
                .to_string(),
        );
    }
    if calls[3].fixture_event.as_deref() != Some("served_typed_lineage_producer")
        || calls[4..]
            .iter()
            .any(|call| call.fixture_event.as_deref() != Some("served_typed_lineage_consumer"))
    {
        return Err(
            "typed lineage fixture did not retain aggregate producer and consumer checkpoints"
                .to_string(),
        );
    }
    validate_stable_rebind_contract(mcp, calls, requested)
}

fn typed_tokens(mcp: &FakeMcpServer) -> Result<serde_json::Map<String, JsonValue>, String> {
    let raw = fs::read_to_string(&mcp.state_path)
        .map_err(|_| "typed lineage fixture state was unavailable".to_string())?;
    let state: JsonValue = serde_json::from_str(&raw)
        .map_err(|_| "typed lineage fixture state was malformed".to_string())?;
    state
        .get("typed_lineage_tokens")
        .and_then(JsonValue::as_object)
        .cloned()
        .ok_or("typed lineage fixture omitted its transient typed selections".to_string())
}

fn token<'a>(
    tokens: &'a serde_json::Map<String, JsonValue>,
    name: &str,
) -> Result<&'a str, String> {
    tokens
        .get(name)
        .and_then(JsonValue::as_str)
        .ok_or("typed lineage fixture omitted a transient selection".to_string())
}

fn terminal_function_name(qualified_name: &str) -> Result<&str, String> {
    qualified_name
        .rsplit_once("::")
        .map(|(_, terminal)| terminal)
        .filter(|terminal| !terminal.is_empty())
        .ok_or("typed lineage fixture did not provide a transformable typed selection".to_string())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn call(name: &str, arguments: JsonValue, fixture_event: &str) -> McpToolCallEvidence {
        McpToolCallEvidence {
            name: name.to_string(),
            arguments,
            delay_ms: (name == "index_repository").then_some(750),
            is_error: false,
            fixture_event: Some(fixture_event.to_string()),
        }
    }

    fn fixture() -> (tempfile::TempDir, FakeMcpServer, Vec<McpToolCallEvidence>) {
        let workspace = tempfile::tempdir().expect("workspace");
        let requested = "temper-v1-scenario";
        let confirmed = "normalized-temper-v1-scenario";
        let root = "crate::fixture::anchor";
        let behavioral = "crate::fixture::anchor_behavior";
        let state_path = workspace.path().join("mcp.jsonl.state.json");
        fs::write(
            &state_path,
            serde_json::json!({
                "projects": {confirmed: {
                    "requested_stable_project": requested,
                    "repo_path": "/workspace/demo",
                    "binding": "current_prepared_checkout"
                }},
                "counters": {"project_creations": 1, "rebinds": 1},
                "typed_lineage_tokens": {"root": root, "behavioral_test": behavioral}
            })
            .to_string(),
        )
        .expect("write fixture state");
        let mcp = FakeMcpServer {
            script_path: workspace.path().join("fake.py"),
            log_path: workspace.path().join("mcp.jsonl"),
            state_path,
            project: "demo".to_string(),
            lifecycle_profile: Some("provider-neutral-anchor-lineage".to_string()),
            safe_tools: vec!["search_graph".to_string()],
            hidden_tools: vec!["index_repository".to_string()],
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
                "served_typed_lineage_producer",
            ),
            call(
                "trace_path",
                serde_json::json!({"project": confirmed, "function_name": "anchor"}),
                "served_typed_lineage_consumer",
            ),
            call(
                "get_code_snippet",
                serde_json::json!({"project": confirmed, "qualified_name": root}),
                "served_typed_lineage_consumer",
            ),
            call(
                "get_code_snippet",
                serde_json::json!({"project": confirmed, "qualified_name": behavioral}),
                "served_typed_lineage_consumer",
            ),
        ];
        (workspace, mcp, calls)
    }

    #[test]
    fn accepts_only_the_later_transformed_descendant_chain() {
        let (_workspace, mcp, calls) = fixture();
        validate(&mcp, &calls).expect("transformed typed chain is accepted");
    }

    #[test]
    fn rejects_unrelated_or_incomplete_consumption() {
        let (_workspace, mcp, mut calls) = fixture();
        calls[4].arguments["function_name"] = JsonValue::String("unrelated".to_string());
        assert!(
            validate(&mcp, &calls)
                .expect_err("unrelated typed target is rejected")
                .contains("approved transformed descendant")
        );

        let (_workspace, mcp, mut calls) = fixture();
        calls.pop();
        assert!(
            validate(&mcp, &calls)
                .expect_err("incomplete evidence is rejected")
                .contains("declared ordered MCP chain")
        );

        let (_workspace, mcp, mut calls) = fixture();
        calls[5].arguments["project"] = JsonValue::String("other-root".to_string());
        assert!(
            validate(&mcp, &calls)
                .expect_err("cross-root source evidence is rejected")
                .contains("confirmed current-root binding")
        );

        let (_workspace, mcp, mut calls) = fixture();
        calls[4].arguments["function_name"] = JsonValue::String(String::new());
        assert!(
            validate(&mcp, &calls)
                .expect_err("malformed transformed selection is rejected")
                .contains("approved transformed descendant")
        );
    }
}
