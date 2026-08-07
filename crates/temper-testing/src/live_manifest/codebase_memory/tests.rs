use std::fs;

use super::*;

fn call(client: &str, name: &str, arguments: JsonValue) -> McpToolCallEvidence {
    McpToolCallEvidence {
        client: client.to_string(),
        name: name.to_string(),
        arguments,
    }
}

#[test]
fn accepts_maintenance_inventory_after_agent_targeted_startup() {
    let calls = vec![
        call(
            MAINTENANCE_MCP_CLIENT,
            "list_projects",
            serde_json::json!({ "limit": 50 }),
        ),
        call(
            AGENT_MCP_CLIENT,
            "index_status",
            serde_json::json!({ "project": "temper-v1-demo" }),
        ),
        call(
            AGENT_MCP_CLIENT,
            "index_repository",
            serde_json::json!({
                "name": "temper-v1-demo",
                "repo_path": "/tmp/demo"
            }),
        ),
        call(
            AGENT_MCP_CLIENT,
            "search_code",
            serde_json::json!({
                "project": "temper-v1-demo",
                "query": "WidgetService"
            }),
        ),
    ];

    validate_mcp_calls(&calls)
        .expect("maintenance inventory must not be mistaken for agent startup discovery");
}

#[test]
fn keeps_interleaved_maintenance_inventory_out_of_agent_contract_checks() {
    let log = tempfile::NamedTempFile::new().expect("temporary MCP log");
    let records = [
        serde_json::json!({
            "pid": 10,
            "tool": "initialize",
            "arguments": {"clientInfo": {"name": MAINTENANCE_MCP_CLIENT}}
        }),
        serde_json::json!({
            "pid": 20,
            "tool": "initialize",
            "arguments": {"clientInfo": {"name": AGENT_MCP_CLIENT}}
        }),
        serde_json::json!({"pid": 10, "tool": "list_projects", "arguments": {"limit": 50}}),
        serde_json::json!({
            "pid": 20,
            "tool": "index_repository",
            "arguments": {"name": "temper-v1-demo", "repo_path": "/tmp/demo"}
        }),
        serde_json::json!({"pid": 10, "tool": "list_projects", "arguments": {"cursor": "50", "limit": 50}}),
        serde_json::json!({
            "pid": 20,
            "tool": "search_code",
            "arguments": {"project": "temper-v1-demo", "query": "WidgetService"}
        }),
    ];
    let contents = records
        .iter()
        .map(serde_json::to_string)
        .collect::<Result<Vec<_>, _>>()
        .expect("serialize MCP log")
        .join("\n");
    fs::write(log.path(), format!("{contents}\n")).expect("write MCP log");

    let calls = logged_tool_calls(log.path()).expect("parse MCP log");
    validate_mcp_calls(&calls)
        .expect("interleaved maintenance inventory must not be attributed to the agent");
}

#[test]
fn rejects_global_inventory_from_an_agent_or_unknown_client() {
    let calls = vec![
        call(
            AGENT_MCP_CLIENT,
            "index_repository",
            serde_json::json!({
                "name": "temper-v1-demo",
                "repo_path": "/tmp/demo"
            }),
        ),
        call(
            AGENT_MCP_CLIENT,
            "search_code",
            serde_json::json!({
                "project": "temper-v1-demo",
                "query": "WidgetService"
            }),
        ),
        call(
            AGENT_MCP_CLIENT,
            "list_projects",
            serde_json::json!({ "limit": 50 }),
        ),
    ];

    let error = validate_mcp_calls(&calls).expect_err("agent inventory must be rejected");
    assert!(error.contains("normal startup called the global project inventory"));
    assert!(error.contains(AGENT_MCP_CLIENT));
}
