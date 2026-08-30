import json
import os
import sys

IMPLEMENTATION = "crate::routing::select_worker"
PARTIAL_IMPLEMENTATION = "temper-v1-private.src.routing.worker_slot"
CALLER = "crate::delivery::dispatch"
FOCUSED_TEST = "crate::tests::keeps_affinity"
SECOND_FOCUSED_TEST = "crate::tests::keeps_affinity_after_retry"
NON_TEST_HELPER = "crate::routing::request_affinity_helper"
SEMANTIC_IMPLEMENTATION = "crate::scheduler::choose_lane"
SEMANTIC_CALLER = "crate::replay::route_renamed_job"
SEMANTIC_FOCUSED_TEST = "crate::tests::renamed_replay_preserves_shard_ownership"
ACTIVE_ROOT_TEST = "crate::tests::replay_uses_selected_lane"
INCIDENTAL_IMPLEMENTATION = "crate::routing::legacy_dispatch_key"
CALL_LOG = os.path.join(os.path.dirname(__file__), "calls.jsonl")
partial_implementation_reads = None

TOOLS = [
    {"name": "search_graph", "description": "Targeted graph search", "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}, "name_pattern": {"type": "string"}, "label": {"type": "string"}, "project": {"type": "string"}}}},
    {"name": "search_code", "description": "Targeted code search", "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}, "pattern": {"type": "string"}, "project": {"type": "string"}}, "required": ["query"]}},
    {"name": "trace_path", "description": "Targeted caller trace", "inputSchema": {"type": "object", "properties": {"function_name": {"type": "string"}, "mode": {"type": "string"}, "direction": {"type": "string"}, "include_tests": {"type": "boolean"}, "depth": {"type": "integer"}, "project": {"type": "string"}}, "required": ["function_name"]}},
    {"name": "get_code_snippet", "description": "Targeted source read", "inputSchema": {"type": "object", "properties": {"qualified_name": {"type": "string"}, "project": {"type": "string"}}, "required": ["qualified_name"]}},
    {"name": "index_status", "description": "Index status", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}}, "required": ["project"]}},
    {"name": "index_repository", "description": "Index repository", "inputSchema": {"type": "object", "properties": {"repo_path": {"type": "string"}, "name": {"type": "string"}}, "required": ["repo_path", "name"]}},
]

def send(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()

def result(request_id, payload):
    send({"jsonrpc": "2.0", "id": request_id, "result": {"content": [{"type": "text", "text": json.dumps(payload)}], "isError": False}})

for line in sys.stdin:
    if not line.strip():
        continue
    request = json.loads(line)
    if "id" not in request:
        continue
    method = request.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": request["id"], "result": {"protocolVersion": "2024-11-05", "serverInfo": {"name": "codebase-memory-mcp", "version": "0.9.0"}, "capabilities": {"tools": {}}}})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": request["id"], "result": {"tools": TOOLS}})
    elif method == "tools/call":
        params = request.get("params") or {}
        name = params.get("name", "")
        arguments = params.get("arguments") or {}
        with open(CALL_LOG, "a", encoding="utf-8") as log:
            log.write(json.dumps({"name": name, "arguments": arguments}, sort_keys=True) + "\n")
        if name == "index_status":
            result(request["id"], {"project": arguments.get("project", ""), "status": "fresh"})
        elif name == "search_graph":
            if arguments.get("query") == "alias retries preserve affinity through worker selection":
                result(request["id"], {"results": [{"qualified_name": PARTIAL_IMPLEMENTATION, "name": "worker_slot", "label": "Function"}, {"qualified_name": CALLER, "name": "dispatch", "label": "Function"}, {"qualified_name": "crate.model.DeliveryAttempt.affinity_topic", "name": "affinity_topic", "label": "Function"}, {"qualified_name": FOCUSED_TEST, "name": "keeps_affinity", "label": "Function", "is_test": True}] + [{"qualified_name": f"crate.decoy.module_{index}.helper_{index}", "name": f"helper_{index}", "label": "Function"} for index in range(10)], "total": 14, "has_more": False})
            elif arguments.get("query") == "worker slot routes alias retry affinity":
                result(request["id"], {"results": [{"qualified_name": PARTIAL_IMPLEMENTATION, "label": "Function"}] + [{"qualified_name": f"crate.narrow.module_{index}.candidate_{index}", "label": "Function"} for index in range(9)], "total": 10, "has_more": False})
            elif arguments.get("name_pattern") == "worker_slot" and arguments.get("label") == "Function":
                partial_implementation_reads = 1
                result(request["id"], {"results": [{"qualified_name": PARTIAL_IMPLEMENTATION, "name": "worker_slot", "label": "Function", "file_path": "src/route.rs"}], "total": 1, "has_more": False})
            elif arguments.get("query") == "renamed job replay shard ownership":
                result(request["id"], {"results": [{"qualified_name": SEMANTIC_IMPLEMENTATION, "name": "choose_lane"}, {"qualified_name": SEMANTIC_CALLER, "name": "route_renamed_job"}, {"qualified_name": ACTIVE_ROOT_TEST, "name": "replay_uses_selected_lane", "is_test": True}]})
            elif arguments.get("query") == "renamed replay shard ownership regression":
                result(request["id"], {"results": [{"qualified_name": SEMANTIC_FOCUSED_TEST, "name": "renamed_replay_preserves_shard_ownership", "is_test": True}]})
            elif arguments.get("query") == "request affinity remains stable across worker selection":
                result(request["id"], {"results": [{"qualified_name": FOCUSED_TEST, "name": "keeps_affinity", "label": "Function", "file_path": "tests/request_affinity.rs", "degree": 1}, {"qualified_name": SECOND_FOCUSED_TEST, "name": "keeps_affinity_after_retry", "label": "Function", "file_path": "tests/request_affinity.rs", "degree": 1}, {"qualified_name": NON_TEST_HELPER, "name": "request_affinity_helper", "label": "Function", "file_path": "src/route.rs", "degree": 2}], "total": 3, "has_more": False})
            elif arguments.get("query") == "request affinity regression without a matching test":
                result(request["id"], {"results": []})
            elif arguments.get("query") == "request affinity regression with ambiguous coverage":
                result(request["id"], {"results": [{"qualified_name": FOCUSED_TEST, "function_name": "different_test_identity", "label": "Function"}]})
            elif arguments.get("query") == "request affinity regression with malformed coverage":
                result(request["id"], {"results": [{"qualified_name": ["not", "an", "identity"]}]})
            elif arguments.get("query") == "request affinity regression returning a non-test helper":
                result(request["id"], {"results": [{"qualified_name": NON_TEST_HELPER, "name": "request_affinity_helper", "label": "Function"}]})
            elif arguments.get("name_pattern") == "dispatch_key":
                result(request["id"], {"results": [{"qualified_name": INCIDENTAL_IMPLEMENTATION, "name": "legacy_dispatch_key"}]})
            else:
                raise AssertionError("unexpected graph search " + str(arguments))
        elif name == "search_code":
            selected = arguments.get("pattern")
            if selected == "partial select worker":
                partial_implementation_reads = 0
                result(request["id"], {"results": [{"qualified_name": "temper-v1-private.src.routing.worker_slot", "name": "worker_slot"}]})
            elif selected == SEMANTIC_IMPLEMENTATION:
                result(request["id"], {"results": [{"qualified_name": SEMANTIC_IMPLEMENTATION, "name": "choose_lane"}]})
            else:
                result(request["id"], {"results": [{"qualified_name": IMPLEMENTATION, "name": "select_worker"}]})
        elif name == "trace_path":
            selected = arguments.get("function_name")
            assert arguments.get("direction") == "inbound"
            if selected == "worker_slot":
                assert partial_implementation_reads is not None
                result(request["id"], {"function": {"qualified_name": "temper-v1-private.src.routing.worker_slot", "name": "worker_slot"}, "direction": "inbound", "complete": True, "callers": [{"qualified_name": CALLER, "name": "dispatch"}]})
            elif selected == IMPLEMENTATION:
                assert not arguments.get("include_tests", False)
                result(request["id"], {"function": {"qualified_name": IMPLEMENTATION, "name": "select_worker"}, "direction": "inbound", "complete": True, "callers": [{"qualified_name": CALLER, "name": "dispatch"}]})
            elif selected == CALLER:
                assert arguments.get("mode") == "calls"
                assert arguments.get("include_tests") is True
                if arguments.get("depth") == 1:
                    result(request["id"], {"function": {"qualified_name": CALLER, "name": "dispatch"}, "mode": "calls", "direction": "inbound", "include_tests": True, "complete": True, "callers": []})
                else:
                    result(request["id"], {"function": {"qualified_name": CALLER, "name": "dispatch"}, "mode": "calls", "direction": "inbound", "include_tests": True, "complete": True, "callers": [{"qualified_name": FOCUSED_TEST, "name": "keeps_affinity", "is_test": True}]})
            elif selected == SEMANTIC_IMPLEMENTATION:
                assert not arguments.get("include_tests", False)
                result(request["id"], {"function": {"qualified_name": SEMANTIC_IMPLEMENTATION, "name": "choose_lane"}, "direction": "inbound", "complete": True, "callers": [{"qualified_name": SEMANTIC_CALLER, "name": "route_renamed_job"}]})
            elif selected == SEMANTIC_CALLER:
                assert arguments.get("mode") == "calls"
                assert arguments.get("include_tests") is True
                result(request["id"], {"function": {"qualified_name": SEMANTIC_CALLER, "name": "route_renamed_job"}, "mode": "calls", "direction": "inbound", "include_tests": True, "complete": True, "callers": []})
            else:
                raise AssertionError("unexpected trace selector " + str(selected))
        elif name == "get_code_snippet":
            selected = arguments.get("qualified_name")
            if selected in ("temper-v1-private.src.routing.worker_slot", "worker_slot"):
                payload = {"qualified_name": "temper-v1-private.src.routing.worker_slot", "name": "worker_slot", "file_path": "ROUTE.md", "source": "partial implementation source"}
                if partial_implementation_reads is not None:
                    partial_implementation_reads += 1
                payload["callers"] = 1 if partial_implementation_reads == 1 else [{"qualified_name": CALLER, "name": "dispatch"}]
                result(request["id"], payload)
            elif selected == IMPLEMENTATION:
                payload = {"qualified_name": IMPLEMENTATION, "name": "select_worker", "file_path": "ROUTE.md", "source": "implementation source"}
                if partial_implementation_reads is not None:
                    partial_implementation_reads += 1
                payload["callers"] = 1 if partial_implementation_reads == 1 else [{"qualified_name": CALLER, "name": "dispatch"}]
                result(request["id"], payload)
            elif selected == CALLER:
                result(request["id"], {"qualified_name": CALLER, "name": "dispatch", "file_path": "ROUTE.md", "source": "caller source", "callees": [{"qualified_name": IMPLEMENTATION, "name": "select_worker"}]})
            elif selected == FOCUSED_TEST:
                result(request["id"], {"qualified_name": FOCUSED_TEST, "name": "keeps_affinity", "file_path": "ROUTE.md", "source": "focused test source", "is_test": True})
            elif selected == SECOND_FOCUSED_TEST:
                result(request["id"], {"qualified_name": SECOND_FOCUSED_TEST, "name": "keeps_affinity_after_retry", "file_path": "ROUTE.md", "source": "second focused test source", "is_test": True})
            elif selected == NON_TEST_HELPER:
                result(request["id"], {"qualified_name": NON_TEST_HELPER, "name": "request_affinity_helper", "file_path": "ROUTE.md", "source": "non-test helper source", "is_test": False})
            elif selected == SEMANTIC_IMPLEMENTATION:
                result(request["id"], {"qualified_name": SEMANTIC_IMPLEMENTATION, "name": "choose_lane", "file_path": "ROUTE.md", "source": "semantic implementation source"})
            elif selected == SEMANTIC_CALLER:
                result(request["id"], {"qualified_name": SEMANTIC_CALLER, "name": "route_renamed_job", "file_path": "ROUTE.md", "source": "semantic caller source", "callees": [{"qualified_name": SEMANTIC_IMPLEMENTATION, "name": "choose_lane"}]})
            elif selected == SEMANTIC_FOCUSED_TEST:
                result(request["id"], {"qualified_name": SEMANTIC_FOCUSED_TEST, "name": "renamed_replay_preserves_shard_ownership", "file_path": "ROUTE.md", "source": "semantic focused test source", "is_test": True})
            elif selected == ACTIVE_ROOT_TEST:
                result(request["id"], {"qualified_name": ACTIVE_ROOT_TEST, "name": "replay_uses_selected_lane", "file_path": "ROUTE.md", "source": "active-root focused test source", "is_test": True})
            elif selected == INCIDENTAL_IMPLEMENTATION:
                result(request["id"], {"qualified_name": INCIDENTAL_IMPLEMENTATION, "name": "legacy_dispatch_key", "file_path": "ROUTE.md", "source": "incidental implementation source"})
            else:
                raise AssertionError("unexpected source selector " + str(selected))
    else:
        send({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32601, "message": "unknown method"}})
