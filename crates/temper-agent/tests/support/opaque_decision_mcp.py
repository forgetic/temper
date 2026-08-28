import json
import sys
import uuid

TOOLS = [
    {"name": "search_graph", "description": "Targeted graph search", "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}, "project": {"type": "string"}}, "required": ["query"]}},
    {"name": "search_code", "description": "Targeted code search", "inputSchema": {"type": "object", "properties": {"pattern": {"type": "string"}, "project": {"type": "string"}, "force_unavailable": {"type": "boolean"}}, "required": ["pattern"]}},
    {"name": "trace_path", "description": "Targeted caller trace", "inputSchema": {"type": "object", "properties": {"function_name": {"type": "string"}, "mode": {"type": "string"}, "direction": {"type": "string"}, "include_tests": {"type": "boolean"}, "project": {"type": "string"}, "force_unavailable": {"type": "boolean"}}, "required": ["function_name"]}},
    {"name": "get_code_snippet", "description": "Targeted source read", "inputSchema": {"type": "object", "properties": {"qualified_name": {"type": "string"}, "project": {"type": "string"}, "force_unavailable": {"type": "boolean"}}, "required": ["qualified_name"]}},
    {"name": "get_architecture", "description": "Bounded architecture query", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}}}},
    {"name": "index_status", "description": "Index status", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}}, "required": ["project"]}},
    {"name": "index_repository", "description": "Stable repository upsert", "inputSchema": {"type": "object", "properties": {"repo_path": {"type": "string"}, "name": {"type": "string"}}, "required": ["repo_path", "name"]}},
]

def opaque():
    return "crate::opaque_" + uuid.uuid4().hex

targets = {name: opaque() for name in ["root", "refinement", "implementation", "caller", "behavior", "selectorless_implementation"]}
fallback_targets = [f"crate.fallback.nonviable_{index}" for index in range(3)]

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
    if name == "get_architecture":
        return {"architecture": "bounded non-progress"}
    if name == "search_graph":
        query = args.get("query")
        if query == "oversized-unretained":
            return {"payload": "x" * 20000}
        if query in {"nonviable-one", "nonviable-two", "nonviable-three"}:
            index = ["nonviable-one", "nonviable-two", "nonviable-three"].index(query)
            return result(qualified_name=fallback_targets[index])
        if query == "unconsumable":
            return result(opaque="PRIVATE-UNCONSUMABLE-SENTINEL")
        return result(
            current_root=targets["root"],
            next=targets["refinement"],
            qualified_name=targets["refinement"],
        )
    if name == "search_code":
        pattern = args.get("pattern")
        if pattern == "selectorless-viable-root":
            return result(
                current_root=targets["root"],
                next=targets["selectorless_implementation"],
                qualified_name=targets["selectorless_implementation"],
            )
        if pattern in {"nonviable-one", "nonviable-two", "nonviable-three"}:
            index = ["nonviable-one", "nonviable-two", "nonviable-three"].index(pattern)
            return result(qualified_name=fallback_targets[index])
        next = targets["implementation"] if pattern == targets["refinement"] else opaque()
        return result(next=next, qualified_name=next)
    if name == "trace_path" and args.get("function_name") == targets["selectorless_implementation"]:
        return {
            "function": {"qualified_name": targets["selectorless_implementation"]},
            "callers": [],
            "results": [{"qualified_name": targets["selectorless_implementation"]}],
        }
    if name == "trace_path" and args.get("function_name") in fallback_targets:
        return {
            "function": {"qualified_name": args.get("function_name")},
            "callers": [],
            "results": [{"qualified_name": args.get("function_name")}],
        }
    if name == "trace_path" and args.get("function_name") == targets["implementation"]:
        return {
            "function": {"qualified_name": targets["implementation"]},
            "callers": [{"qualified_name": targets["caller"]}],
            "results": [{"next": targets["caller"], "qualified_name": targets["caller"]}],
        }
    if name == "trace_path" and args.get("function_name") == targets["caller"] and args.get("include_tests") is True:
        return {
            "function": {"qualified_name": targets["caller"]},
            "callers": [{"qualified_name": targets["behavior"], "is_test": True}],
            "results": [{"next": targets["behavior"], "qualified_name": targets["behavior"], "is_test": True}],
        }
    if name == "trace_path":
        return result(next=opaque(), qualified_name=opaque())
    if name == "get_code_snippet" and args.get("qualified_name") == targets["selectorless_implementation"]:
        return result(
            next=targets["selectorless_implementation"],
            qualified_name=targets["selectorless_implementation"],
            file_path="EVIDENCE.md",
            source=opaque(),
            implementation_source=opaque(),
        )
    if name == "get_code_snippet" and args.get("qualified_name") in fallback_targets:
        return result(
            qualified_name=args.get("qualified_name"),
            file_path="EVIDENCE.md",
            source=opaque(),
            implementation_source=opaque(),
        )
    if name == "get_code_snippet" and args.get("qualified_name") == targets["implementation"]:
        return result(
            next=targets["implementation"],
            qualified_name=targets["implementation"],
            file_path="EVIDENCE.md",
            source=opaque(),
            implementation_source=opaque(),
        )
    if name == "get_code_snippet" and args.get("qualified_name") == targets["caller"]:
        return result(
            next=targets["caller"],
            qualified_name=targets["caller"],
            file_path="EVIDENCE.md",
            source=opaque(),
            caller_model=opaque(),
        )
    if name == "get_code_snippet" and args.get("qualified_name") == targets["behavior"]:
        return result(
            qualified_name=targets["behavior"],
            file_path="EVIDENCE.md",
            source=opaque(),
            behavioral_test=opaque(),
        )
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
        if args.get("force_unavailable"):
            rpc_result(request["id"], {"content": [{"type": "text", "text": "provider unavailable"}], "isError": True})
        else:
            tool_result(request["id"], response(name, args))
    else:
        send({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32601, "message": "unknown method"}})
