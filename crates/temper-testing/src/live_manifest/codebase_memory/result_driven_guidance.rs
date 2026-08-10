use std::fs;

use serde_json::Value as JsonValue;

use super::stable_rebind::{confirmed_project_from_calls, validate_stable_rebind_contract};
use super::{FakeMcpServer, McpToolCallEvidence};

/// Validate a decision chain whose dependent values are minted by the live
/// fixture. Values remain in ephemeral fixture state; durable evidence contains
/// only the safe aggregate facts emitted by the manifest runner.
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
        return Err("result-driven fixture requires its declared ordered MCP chain".to_string());
    }
    if calls.iter().any(|call| call.is_error) {
        return Err("result-driven fixture recorded an unsuccessful declared MCP call".to_string());
    }

    let requested = calls[1]
        .arguments
        .get("name")
        .and_then(JsonValue::as_str)
        .filter(|name| name.starts_with("temper-v1-"))
        .ok_or("result-driven fixture upsert did not use a stable provider identity")?;
    if calls[0]
        .arguments
        .get("project")
        .and_then(JsonValue::as_str)
        != Some(requested)
        || calls[0].fixture_event.as_deref() != Some("fresh_prior_binding")
    {
        return Err(
            "result-driven fixture did not begin with targeted stable-key discovery".to_string(),
        );
    }
    let confirmed = confirmed_project_from_calls(calls, requested)?;
    if calls[3..].iter().any(|call| {
        call.arguments.get("project").and_then(JsonValue::as_str) != Some(confirmed.as_str())
    }) {
        return Err(
            "result-driven fixture did not use the confirmed current-root provider identity"
                .to_string(),
        );
    }

    let tokens = decision_tokens(mcp)?;
    for (call, field, token) in [
        (&calls[4], "pattern", "refinement"),
        (&calls[5], "function_name", "trace"),
        (&calls[6], "qualified_name", "implementation"),
        (&calls[7], "qualified_name", "behavioral_test"),
    ] {
        let expected = tokens
            .get(token)
            .and_then(JsonValue::as_str)
            .ok_or("result-driven fixture omitted a minted dependent value")?;
        if call.arguments.get(field).and_then(JsonValue::as_str) != Some(expected) {
            return Err(
                "result-driven fixture accepted a consumer that did not use its preceding result"
                    .to_string(),
            );
        }
    }
    if calls[3].fixture_event.as_deref() != Some("served_result_driven_producer")
        || calls[4..]
            .iter()
            .any(|call| call.fixture_event.as_deref() != Some("served_result_derived_consumer"))
    {
        return Err(
            "result-driven fixture did not retain producer/consumer decision checkpoints"
                .to_string(),
        );
    }
    validate_stable_rebind_contract(mcp, calls, requested)
}

fn decision_tokens(mcp: &FakeMcpServer) -> Result<serde_json::Map<String, JsonValue>, String> {
    let raw = fs::read_to_string(&mcp.state_path).map_err(|error| {
        format!(
            "read result-driven fixture state {}: {error}",
            mcp.state_path.display()
        )
    })?;
    let state: JsonValue = serde_json::from_str(&raw).map_err(|error| {
        format!(
            "parse result-driven fixture state {}: {error}",
            mcp.state_path.display()
        )
    })?;
    state
        .get("decision_tokens")
        .and_then(JsonValue::as_object)
        .cloned()
        .ok_or("result-driven fixture state omitted opaque decision values".to_string())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn call(name: &str, arguments: JsonValue, fixture_event: &str) -> McpToolCallEvidence {
        McpToolCallEvidence {
            name: name.to_string(),
            arguments,
            delay_ms: if name == "index_repository" {
                Some(750)
            } else {
                None
            },
            is_error: false,
            fixture_event: Some(fixture_event.to_string()),
        }
    }

    fn fixture() -> (tempfile::TempDir, FakeMcpServer, Vec<McpToolCallEvidence>) {
        let workspace = tempfile::tempdir().expect("workspace");
        let requested = "temper-v1-opaque";
        let confirmed = "normalized-temper-v1-opaque";
        let state_path = workspace.path().join("mcp.jsonl.state.json");
        fs::write(
            &state_path,
            serde_json::json!({
                "projects": {
                    confirmed: {
                        "requested_stable_project": requested,
                        "repo_path": "/workspace/demo",
                        "binding": "current_prepared_checkout"
                    }
                },
                "counters": {"project_creations": 1, "rebinds": 1},
                "decision_tokens": {
                    "root": "opaque-root",
                    "refinement": "opaque-refinement",
                    "trace": "opaque-trace",
                    "implementation": "opaque-implementation",
                    "behavioral_test": "opaque-behavior"
                }
            })
            .to_string(),
        )
        .expect("write fixture state");
        let mcp = FakeMcpServer {
            script_path: workspace.path().join("fake.py"),
            log_path: workspace.path().join("mcp.jsonl"),
            state_path,
            project: "demo".to_string(),
            lifecycle_profile: Some("result-driven-decision-guidance".to_string()),
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
                "served_result_driven_producer",
            ),
            call(
                "search_code",
                serde_json::json!({"project": confirmed, "pattern": "opaque-refinement"}),
                "served_result_derived_consumer",
            ),
            call(
                "trace_path",
                serde_json::json!({"project": confirmed, "function_name": "opaque-trace"}),
                "served_result_derived_consumer",
            ),
            call(
                "get_code_snippet",
                serde_json::json!({"project": confirmed, "qualified_name": "opaque-implementation"}),
                "served_result_derived_consumer",
            ),
            call(
                "get_code_snippet",
                serde_json::json!({"project": confirmed, "qualified_name": "opaque-behavior"}),
                "served_result_derived_consumer",
            ),
        ];
        (workspace, mcp, calls)
    }

    #[test]
    fn accepts_only_minted_later_turn_consumers() {
        let (_workspace, mcp, calls) = fixture();
        validate(&mcp, &calls).expect("minted consumer chain is accepted");
    }

    #[test]
    fn rejects_an_otherwise_successful_unrelated_consumer() {
        let (_workspace, mcp, mut calls) = fixture();
        calls[4].arguments["pattern"] = JsonValue::String("opaque-unrelated".to_string());

        let error = validate(&mcp, &calls).expect_err("unrelated consumer must be rejected");
        assert!(error.contains("preceding result"));
    }
}
