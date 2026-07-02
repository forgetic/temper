use super::*;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::process::{Command as StdCommand, Stdio as StdStdio};
use std::time::Duration;

fn fake_server_script() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(
            dir.path().join("fake_mcp.py"),
            r#"
import json
import sys
import time

mode = sys.argv[1] if len(sys.argv) > 1 else "normal"
if mode == "hang":
    time.sleep(60)
    sys.exit(0)

TOOLS = [
    {"name": "search_code", "description": "Search code", "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}},
    {"name": "get_architecture", "description": "Architecture", "inputSchema": {"type": "object", "properties": {}}},
    {"name": "delete_project", "description": "Danger", "inputSchema": {"type": "object", "properties": {}}},
]

def send(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    if not line.strip():
        continue
    request = json.loads(line)
    if "id" not in request:
        continue
    method = request.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": request["id"], "result": {"protocolVersion": "2024-11-05", "serverInfo": {"name": "fake", "version": "1"}, "capabilities": {"tools": {}}}})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": request["id"], "result": {"tools": TOOLS}})
    elif method == "tools/call":
        params = request.get("params", {})
        name = params.get("name")
        args = params.get("arguments") or {}
        send({"jsonrpc": "2.0", "id": request["id"], "result": {"content": [{"type": "text", "text": f"called {name} with {json.dumps(args, sort_keys=True)}"}], "isError": False}})
    else:
        send({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32601, "message": "unknown method"}})
"#,
        )
        .expect("write fake server");
    dir
}

fn fake_command(dir: &tempfile::TempDir, mode: Option<&str>) -> StdioMcpServerConfig {
    let mut args = vec!["-u".to_string(), script_path(dir).display().to_string()];
    if let Some(mode) = mode {
        args.push(mode.to_string());
    }
    StdioMcpServerConfig::new("python3", args)
        .with_startup_timeout(Duration::from_secs(1))
        .with_call_timeout(Duration::from_secs(2))
}

fn script_path(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("fake_mcp.py")
}

fn process_exists(pid: u32) -> bool {
    StdCommand::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stderr(StdStdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[test]
fn codebase_memory_bridge_mcp_initializes_lists_and_calls() {
    let dir = fake_server_script();
    temper_agent_io::block_on(async move {
        let client = StdioMcpClient::connect(fake_command(&dir, None))
            .await
            .expect("connect fake MCP server");
        let tools = client
            .list_tools(Duration::from_secs(1))
            .await
            .expect("list tools");
        assert!(tools.iter().any(|tool| tool.name == "search_code"));
        assert!(tools.iter().any(|tool| tool.name == "delete_project"));

        let output = client
            .call_tool(
                "search_code",
                json!({ "query": "needle" }),
                client.call_timeout(),
            )
            .await
            .expect("call tool");
        assert!(!output.is_error);
        assert!(output.text.contains("called search_code"));
        assert!(output.text.contains("needle"));
    });
}

#[test]
fn codebase_memory_bridge_mcp_startup_timeout_kills_hung_server() {
    let dir = fake_server_script();
    temper_agent_io::block_on(async move {
        let error = match StdioMcpClient::connect(fake_command(&dir, Some("hang"))).await {
            Ok(_) => panic!("hung server times out"),
            Err(error) => error,
        };
        assert!(matches!(error, McpError::Timeout { method, .. } if method == "initialize"));
    });
}

#[test]
fn codebase_memory_bridge_mcp_child_exits_when_client_drops() {
    let dir = fake_server_script();
    let pid = temper_agent_io::block_on(async move {
        let client = StdioMcpClient::connect(fake_command(&dir, None))
            .await
            .expect("connect fake MCP server");
        let pid = client.child_id();
        assert!(process_exists(pid));
        pid
    });

    for _ in 0..50 {
        if !process_exists(pid) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("MCP child process {pid} still exists after client drop");
}
