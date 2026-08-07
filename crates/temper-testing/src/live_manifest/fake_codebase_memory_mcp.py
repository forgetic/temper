import json
import sys
import time

LOG_PATH = sys.argv[1]
REPO_ROOT = sys.argv[2]
FIXTURE_PROJECT = sys.argv[3]
SAFE_TOOLS = set(json.loads(sys.argv[4]))
HIDDEN_TOOLS = set(json.loads(sys.argv[5]))
READINESS_DELAY_MS = int(sys.argv[6])
FORCED_FAILURE_TOOL = sys.argv[7]
FORCED_FAILURE_AFTER_CALLS = int(sys.argv[8])
GRAPH_CALLS = 0

TOOLS = [
    {
        "name": "search_graph",
        "description": "Search indexed symbols and relationships",
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "project": {"type": "string"},
            },
            "required": ["query"],
        },
    },
    {
        "name": "list_projects",
        "description": "List indexed projects",
        "inputSchema": {"type": "object", "properties": {}},
    },
    {
        "name": "index_status",
        "description": "Read targeted index status",
        "inputSchema": {
            "type": "object",
            "properties": {"project": {"type": "string"}},
            "required": ["project"],
        },
    },
    {
        "name": "index_repository",
        "description": "Internal stable indexing hook; never model-callable",
        "inputSchema": {
            "type": "object",
            "properties": {
                "repo_path": {"type": "string"},
                "name": {"type": "string"},
            },
            "required": ["repo_path"],
        },
    },
    {
        "name": "delete_project",
        "description": "Unsafe destructive tool that Temper must filter",
        "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}}},
    },
]
TOOLS = [tool for tool in TOOLS if tool["name"] in SAFE_TOOLS | HIDDEN_TOOLS]


def send(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()


def log_tool(name, arguments, delay_ms=None, is_error=False):
    value = {
        "tool": name,
        "arguments": arguments,
        "fixture_project": FIXTURE_PROJECT,
        "is_error": is_error,
    }
    if delay_ms is not None:
        value["delay_ms"] = delay_ms
    with open(LOG_PATH, "a", encoding="utf-8") as handle:
        handle.write(json.dumps(value, sort_keys=True) + "\n")


def text_result(text, is_error=False):
    return {"content": [{"type": "text", "text": text}], "isError": is_error}


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
                "serverInfo": {"name": "codebase-memory-mcp", "version": "0.9.0"},
                "capabilities": {"tools": {}},
            },
        })
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": request["id"], "result": {"tools": TOOLS}})
    elif method == "tools/call":
        params = request.get("params") or {}
        name = params.get("name")
        arguments = params.get("arguments") or {}
        if name == "index_status":
            log_tool(name, arguments, is_error=True)
            result = text_result(
                json.dumps({"project": arguments.get("project", ""), "status": "missing"}),
                True,
            )
        elif name == "index_repository":
            time.sleep(READINESS_DELAY_MS / 1000)
            log_tool(name, arguments, delay_ms=READINESS_DELAY_MS)
            result = text_result(json.dumps({
                "project": arguments.get("name", ""),
                "repo_path": arguments.get("repo_path", REPO_ROOT),
                "status": "fresh",
            }))
        elif name == "search_graph":
            GRAPH_CALLS += 1
            is_error = (
                name == FORCED_FAILURE_TOOL
                and GRAPH_CALLS > FORCED_FAILURE_AFTER_CALLS
            )
            log_tool(name, arguments, is_error=is_error)
            if is_error:
                result = text_result(
                    "fixture backend unavailable Authorization: Bearer MCP-FIXTURE-SECRET",
                    True,
                )
            else:
                result = text_result(
                    "FAKE_MCP_GRAPH_RESULT implementation=src/lib.rs::retry_worker_topic "
                    "focused_test=tests/retry_affinity.rs::alias_retries_keep_the_original_ordered_worker\n"
                    + ("x" * 20_000)
                )
        else:
            log_tool(name, arguments, is_error=True)
            result = text_result("UNSAFE_TOOL_CALLED " + str(name), is_error=True)
        send({"jsonrpc": "2.0", "id": request["id"], "result": result})
    else:
        send({
            "jsonrpc": "2.0",
            "id": request["id"],
            "error": {"code": -32601, "message": "unknown method"},
        })
