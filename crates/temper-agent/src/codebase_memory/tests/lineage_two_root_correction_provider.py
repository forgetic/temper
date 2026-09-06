import json
import os
import sys

log_path = sys.argv[2] if len(sys.argv) > 2 else ""

TOOLS = [
    {"name": "search_graph", "description": "Search graph", "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}, "project": {"type": "string"}}}},
    {"name": "trace_path", "description": "Trace graph calls", "inputSchema": {"type": "object", "properties": {"function_name": {"type": "string"}, "project": {"type": "string"}, "mode": {"type": "string"}, "direction": {"type": "string"}, "include_tests": {"type": "boolean"}}, "required": ["function_name"]}},
    {"name": "get_code_snippet", "description": "Read indexed source", "inputSchema": {"type": "object", "properties": {"qualified_name": {"type": "string"}, "project": {"type": "string"}, "include_neighbors": {"type": "boolean"}}, "required": ["qualified_name"]}},
    {"name": "index_status", "description": "Index status", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}}, "required": ["project"]}},
    {"name": "index_repository", "description": "Stable repository upsert", "inputSchema": {"type": "object", "properties": {"repo_path": {"type": "string"}, "name": {"type": "string"}}, "required": ["repo_path", "name"]}},
]


def send(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()


def tool_result(request_id, payload, is_error=False):
    send({
        "jsonrpc": "2.0",
        "id": request_id,
        "result": {
            "content": [{"type": "text", "text": json.dumps(payload) if not is_error else payload}],
            "structuredContent": payload if not is_error else None,
            "isError": is_error,
        },
    })


def log_tool(name, arguments):
    if log_path:
        with open(log_path, "a", encoding="utf-8") as handle:
            handle.write(json.dumps({"name": name, "arguments": arguments, "pid": os.getpid()}, sort_keys=True) + "\n")


def search_graph(query):
    if query == "routing implementation authority":
        results = [
            {"name": "affinity_topic", "qualified_name": "temper-v1-production.src.model.affinity_topic", "label": "Function", "file_path": "src/model.rs"},
            {"name": "worker_for", "qualified_name": "temper-v1-production.src.route.worker_for", "label": "Function", "file_path": "src/route.rs"},
            {"name": "worker_slot", "qualified_name": "temper-v1-production.src.route.worker_slot", "label": "Function", "file_path": "src/route.rs"},
        ]
    elif query == "focused routing regression":
        results = [
            {"name": "keeps_worker_affinity", "qualified_name": "temper-v1-production.tests.route.keeps_worker_affinity", "label": "Function", "file_path": "tests/route.rs", "is_test": True},
        ]
    else:
        return None
    return {"total": len(results), "has_more": False, "results": results, "provider_note": "PRIVATE-TWO-ROOT-PROVIDER-TEXT"}


def trace_path(symbol):
    if symbol == "affinity_topic":
        callers = [
            {"name": "worker_slot", "qualified_name": "temper-v1-production.src.route.worker_slot"},
            {"name": "worker_for", "qualified_name": "temper-v1-production.src.route.worker_for"},
        ]
    elif symbol == "worker_slot":
        callers = [
            {"name": "worker_for", "qualified_name": "temper-v1-production.src.route.worker_for"},
            {"name": "affinity_topic", "qualified_name": "temper-v1-production.src.model.affinity_topic"},
        ]
    else:
        return None
    return {"function": symbol, "direction": "inbound", "mode": "calls", "callers": callers, "provider_note": "PRIVATE-TWO-ROOT-PROVIDER-TEXT"}


def get_code_snippet(symbol):
    paths = {
        "temper-v1-production.src.model.affinity_topic": "src/model.rs",
        "temper-v1-production.src.route.worker_slot": "src/route.rs",
        "temper-v1-production.src.route.worker_for": "src/route.rs",
        "temper-v1-production.tests.route.keeps_worker_affinity": "tests/route.rs",
        "keeps_worker_affinity": "tests/route.rs",
    }
    if symbol not in paths:
        return None
    leaf = symbol.rsplit(".", 1)[-1]
    qualified_symbol = "temper-v1-production.tests.route.keeps_worker_affinity" if symbol == "keeps_worker_affinity" else symbol
    payload = {
        "name": leaf,
        "qualified_name": qualified_symbol,
        "label": "Function",
        "file_path": paths[symbol],
        "source": f"// PRIVATE-TWO-ROOT-SOURCE\nfn {leaf}() {{}}",
        "callers": 0,
        "callees": 0,
    }
    if ".tests." in qualified_symbol:
        payload["is_test"] = True
    return payload


for line in sys.stdin:
    if not line.strip():
        continue
    request = json.loads(line)
    if "id" not in request:
        continue
    method = request.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": request["id"], "result": {"protocolVersion": "2024-11-05", "serverInfo": {"name": "codebase-memory-mcp", "version": "0.10.8"}, "capabilities": {"tools": {}}}})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": request["id"], "result": {"tools": TOOLS}})
    elif method == "tools/call":
        params = request.get("params", {})
        name = params.get("name")
        arguments = params.get("arguments") or {}
        log_tool(name, arguments)
        if name == "search_graph":
            payload = search_graph(arguments.get("query"))
        elif name == "trace_path":
            payload = trace_path(arguments.get("function_name"))
        elif name == "get_code_snippet":
            payload = get_code_snippet(arguments.get("qualified_name"))
        elif name == "index_status":
            payload = {"project": arguments.get("project"), "status": "fresh"}
        elif name == "index_repository":
            payload = {"project": arguments.get("name"), "status": "indexed"}
        else:
            payload = None
        if payload is None:
            tool_result(request["id"], "invalid argument", True)
        else:
            tool_result(request["id"], payload)
    else:
        send({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32601, "message": "unknown method"}})
