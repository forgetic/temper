import json
import sys
import uuid

TOOLS = [
    {"name": "search_graph", "description": "Targeted graph search", "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}, "project": {"type": "string"}}, "required": ["query"]}},
    {"name": "search_code", "description": "Targeted code search", "inputSchema": {"type": "object", "properties": {"pattern": {"type": "string"}, "project": {"type": "string"}}, "required": ["pattern"]}},
    {"name": "trace_path", "description": "Targeted caller trace", "inputSchema": {"type": "object", "properties": {"function_name": {"type": "string"}, "project": {"type": "string"}}, "required": ["function_name"]}},
    {"name": "get_code_snippet", "description": "Targeted source read", "inputSchema": {"type": "object", "properties": {"qualified_name": {"type": "string"}, "project": {"type": "string"}}, "required": ["qualified_name"]}},
    {"name": "index_status", "description": "Index status", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}}, "required": ["project"]}},
    {"name": "index_repository", "description": "Stable repository upsert", "inputSchema": {"type": "object", "properties": {"repo_path": {"type": "string"}, "name": {"type": "string"}}, "required": ["repo_path", "name"]}},
]

def opaque():
    return "crate::opaque_" + uuid.uuid4().hex

targets = {name: opaque() for name in ["root", "refinement", "trace", "implementation", "behavior"]}

def send(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()

def rpc_result(request_id, payload):
    send({"jsonrpc": "2.0", "id": request_id, "result": payload})

def tool_result(request_id, payload):
    rpc_result(request_id, {"content": [{"type": "text", "text": json.dumps(payload)}], "isError": False})

def result(**values):
    return {"results": [values]}

def response(name, args):
    if name == "index_status":
        return {"status": "fresh"}
    if name == "search_graph":
        if args.get("query") == "unconsumable":
            return result(opaque="PRIVATE-UNCONSUMABLE-SENTINEL")
        return result(
            current_root=targets["root"],
            next=targets["refinement"],
            qualified_name=targets["refinement"],
        )
    if name == "search_code":
        next = targets["trace"] if args.get("pattern") == targets["refinement"] else opaque()
        return result(next=next, qualified_name=next)
    if name == "trace_path":
        next = targets["implementation"] if args.get("function_name") == targets["trace"] else opaque()
        return result(
            next=next,
            qualified_name=next,
            caller_model=opaque() if args.get("function_name") == targets["trace"] else None,
        )
    if name == "get_code_snippet" and args.get("qualified_name") == targets["implementation"]:
        return result(
            next=targets["behavior"],
            qualified_name=targets["behavior"],
            implementation_source=opaque(),
        )
    if name == "get_code_snippet" and args.get("qualified_name") == targets["behavior"]:
        return result(qualified_name=targets["behavior"], behavioral_test=opaque())
    return result(qualified_name=opaque(), evidence=opaque())

for line in sys.stdin:
    if not line.strip():
        continue
    request = json.loads(line)
    if "id" not in request:
        continue
    method = request.get("method")
    if method == "initialize":
        rpc_result(request["id"], {"protocolVersion": "2024-11-05", "serverInfo": {"name": "codebase-memory-mcp", "version": "0.9.0"}, "capabilities": {"tools": {}}})
    elif method == "tools/list":
        rpc_result(request["id"], {"tools": TOOLS})
    elif method == "tools/call":
        params = request.get("params", {})
        name = params.get("name")
        args = params.get("arguments") or {}
        if name == "search_code" and args.get("force_unavailable"):
            rpc_result(request["id"], {"content": [{"type": "text", "text": "provider unavailable"}], "isError": True})
        else:
            tool_result(request["id"], response(name, args))
    else:
        send({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32601, "message": "unknown method"}})
