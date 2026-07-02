use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct McpToolCallEvidence {
    pub name: String,
    pub arguments: Value,
}

pub struct FakeMcpServer {
    pub script_path: PathBuf,
    pub log_path: PathBuf,
}

pub fn write_fake_server(dir: &Path) -> Result<FakeMcpServer, String> {
    let script_path = dir.join("fake_codebase_memory_mcp.py");
    let log_path = dir.join("mcp-calls.jsonl");
    fs::write(&script_path, FAKE_MCP_SCRIPT)
        .map_err(|error| format!("write fake MCP server {}: {error}", script_path.display()))?;
    fs::write(&log_path, "")
        .map_err(|error| format!("create MCP call log {}: {error}", log_path.display()))?;
    Ok(FakeMcpServer {
        script_path,
        log_path,
    })
}

pub fn logged_tool_calls(path: &Path) -> Result<Vec<McpToolCallEvidence>, String> {
    let raw = fs::read_to_string(path)
        .map_err(|error| format!("read MCP call log {}: {error}", path.display()))?;
    let calls = raw
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| {
            let value = serde_json::from_str::<Value>(line).ok()?;
            let name = value.get("tool")?.as_str()?.to_string();
            let arguments = value.get("arguments").cloned().unwrap_or(Value::Null);
            Some(McpToolCallEvidence { name, arguments })
        })
        .collect::<Vec<_>>();
    Ok(calls)
}

const FAKE_MCP_SCRIPT: &str = r#"
import json
import sys

LOG_PATH = sys.argv[1]
REPO_ROOT = sys.argv[2]
ACTUAL_PROJECT = sys.argv[3]

TOOLS = [
    {
        "name": "search_code",
        "description": "Search indexed code",
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "project": {"type": "string"}
            },
            "required": ["query"]
        }
    },
    {
        "name": "list_projects",
        "description": "List indexed projects",
        "inputSchema": {"type": "object", "properties": {}}
    },
    {
        "name": "index_status",
        "description": "Read index status",
        "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}}}
    },
    {
        "name": "index_repository",
        "description": "Internal indexing hook; must never be model-callable",
        "inputSchema": {"type": "object", "properties": {"repo_path": {"type": "string"}}}
    },
    {
        "name": "delete_project",
        "description": "Unsafe destructive tool that Temper must filter",
        "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}}}
    }
]


def send(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()


def log(value):
    with open(LOG_PATH, "a", encoding="utf-8") as handle:
        handle.write(json.dumps(value, sort_keys=True) + "\n")


def text_result(text, is_error=False):
    return {"content": [{"type": "text", "text": text}], "isError": is_error}


def project_list(stale=True):
    return json.dumps({
        "projects": [{
            "name": ACTUAL_PROJECT,
            "root_path": REPO_ROOT,
            "stale": stale
        }]
    })


for line in sys.stdin:
    if not line.strip():
        continue
    request = json.loads(line)
    if "id" not in request:
        continue
    method = request.get("method")
    if method == "initialize":
        send({
            "jsonrpc": "2.0",
            "id": request["id"],
            "result": {
                "protocolVersion": "2024-11-05",
                "serverInfo": {"name": "fake-codebase-memory", "version": "1"},
                "capabilities": {"tools": {}}
            }
        })
    elif method == "tools/list":
        log({"method": method})
        send({"jsonrpc": "2.0", "id": request["id"], "result": {"tools": TOOLS}})
    elif method == "tools/call":
        params = request.get("params") or {}
        name = params.get("name")
        args = params.get("arguments") or {}
        log({"method": method, "tool": name, "arguments": args})
        if name == "list_projects":
            result = text_result(project_list(stale=True))
        elif name == "index_repository":
            result = text_result(json.dumps({
                "projects": [{
                    "name": ACTUAL_PROJECT,
                    "root_path": args.get("repo_path", REPO_ROOT),
                    "stale": False
                }]
            }))
        elif name == "search_code":
            result = text_result(
                "FAKE_MCP_SEARCH_RESULT project={} query={}".format(
                    args.get("project", ""), args.get("query", "")
                )
            )
        elif name == "index_status":
            result = text_result("fresh")
        else:
            result = text_result("UNSAFE_TOOL_CALLED " + str(name), is_error=True)
        send({"jsonrpc": "2.0", "id": request["id"], "result": result})
    else:
        send({
            "jsonrpc": "2.0",
            "id": request["id"],
            "error": {"code": -32601, "message": "unknown method"}
        })
"#;
