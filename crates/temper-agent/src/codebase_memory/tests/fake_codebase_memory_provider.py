import fcntl
import json
import os
import sys
import time

mode = sys.argv[1] if len(sys.argv) > 1 else "normal"
log_path = sys.argv[2] if len(sys.argv) > 2 else ""
if mode == "hang":
    time.sleep(60)
    sys.exit(0)

provider_name = "other-provider" if mode == "incompatible-name" else "codebase-memory-mcp"
provider_version = "0.9.0" if mode == "incompatible-version" else "0.10.8"
capabilities = {} if mode == "incompatible-capability" else {"tools": {}}

index_properties = {
    "repo_path": {"type": "string"},
    "name": {"type": "string"},
}
if mode == "incompatible-schema":
    del index_properties["name"]

search_project_property = "repo" if mode == "repo-schema" else "project"

state_path = f"{log_path}.state.json" if log_path else ""

TOOLS = [
    {"name": "search_code", "description": "Search indexed code", "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}, search_project_property: {"type": "string"}}, "required": ["query", search_project_property] if mode == "repo-schema" else ["query"]}},
    {"name": "get_architecture", "description": "Summarize architecture", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}}}},
    {"name": "get_code_snippet", "description": "Read indexed source", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}, "qualified_name": {"type": "string"}, "include_neighbors": {"type": "boolean"}}, "required": ["qualified_name"]}},
    {"name": "search_graph", "description": "Search graph", "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}, "name_pattern": {"type": "string"}, "project": {"type": "string"}}}},
    {"name": "trace_path", "description": "Trace graph calls", "inputSchema": {"type": "object", "properties": {"function_name": {"type": "string"}, "project": {"type": "string"}, "mode": {"type": "string"}, "direction": {"type": "string"}, "include_tests": {"type": "boolean"}}}},
    {"name": "list_projects", "description": "List projects", "inputSchema": {"type": "object", "properties": {}}},
    {"name": "index_status", "description": "Index status", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}}, "required": ["project"]}},
    {"name": "detect_changes", "description": "Detect changes", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}}}},
    {"name": "delete_project", "description": "Delete project", "inputSchema": {"type": "object", "properties": {}}},
    {"name": "manage_adr", "description": "Write ADRs", "inputSchema": {"type": "object", "properties": {}}},
    {"name": "ingest_traces", "description": "Ingest traces", "inputSchema": {"type": "object", "properties": {}}},
    {"name": "query_graph", "description": "Raw graph query", "inputSchema": {"type": "object", "properties": {}}},
    {"name": "index_repository", "description": "Stable repository upsert", "inputSchema": {"type": "object", "properties": index_properties, "required": ["repo_path"]}},
]

def send(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()

def log_tool(name, args):
    if not log_path:
        return
    with open(log_path, "a", encoding="utf-8") as handle:
        handle.write(json.dumps({"name": name, "arguments": args, "pid": os.getpid()}, sort_keys=True) + "\n")

def with_provider_state(update):
    if not state_path:
        return update({"projects": {}, "counters": {}})
    lock_path = f"{state_path}.lock"
    with open(lock_path, "a+", encoding="utf-8") as lock:
        fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
        try:
            with open(state_path, "r", encoding="utf-8") as handle:
                state = json.load(handle)
        except (FileNotFoundError, json.JSONDecodeError):
            state = {"projects": {}, "counters": {}}
        state.setdefault("projects", {})
        state.setdefault("aliases", {})
        state.setdefault("counters", {})
        result = update(state)
        temporary = f"{state_path}.tmp.{os.getpid()}"
        with open(temporary, "w", encoding="utf-8") as handle:
            json.dump(state, handle, sort_keys=True)
        os.replace(temporary, state_path)
        return result

def provider_identity(project):
    return f"normalized-{project}" if mode == "normalized" else project

def indexed_binding(project, count_status=True):
    def update(state):
        if count_status:
            counters = state["counters"]
            counters["index_status"] = counters.get("index_status", 0) + 1
        actual = state["aliases"].get(project, project)
        return actual, state["projects"].get(actual)
    return with_provider_state(update)

def stable_upsert(project, repo_path):
    actual = provider_identity(project)
    def update(state):
        counters = state["counters"]
        counters["index_repository"] = counters.get("index_repository", 0) + 1
        counters["project_creations"] = counters.get("project_creations", 0) + int(actual not in state["projects"])
        state["aliases"][project] = actual
        state["projects"][actual] = {"repo_path": repo_path}
    with_provider_state(update)
    return actual

def bound_snippet(project):
    _actual, binding = indexed_binding(project, count_status=False)
    repo_path = binding.get("repo_path") if binding else None
    if not repo_path:
        return None
    file_path = os.path.join(repo_path, "src", "lib.rs")
    try:
        with open(file_path, "r", encoding="utf-8") as source_file:
            source = source_file.read()
    except OSError:
        return None
    return {"source": source, "file_path": file_path}

def local_snippet(symbol, file_path, qualified_symbol=None):
    leaf = symbol.rsplit(".", 1)[-1].rsplit("::", 1)[-1]
    with open(file_path, "r", encoding="utf-8", newline="") as handle:
        lines = handle.readlines()
    for number, line in enumerate(lines, 1):
        if line.lstrip().startswith(f"fn {leaf}("):
            return {
                "name": leaf,
                "qualified_name": qualified_symbol or symbol,
                "file_path": file_path,
                "start_line": number,
                "end_line": number,
                "source": line,
            }
    raise AssertionError(f"fixture omitted declared source: {file_path}:{leaf}")


def tool_result(request_id, payload, is_error=False, structured=None):
    result = {"content": [{"type": "text", "text": payload}], "isError": is_error}
    if structured is not None:
        result["structuredContent"] = structured
    send({"jsonrpc": "2.0", "id": request_id, "result": result})

for line in sys.stdin:
    if not line.strip():
        continue
    request = json.loads(line)
    if "id" not in request:
        continue
    method = request.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": request["id"], "result": {"protocolVersion": "2024-11-05", "serverInfo": {"name": provider_name, "version": provider_version}, "capabilities": capabilities}})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": request["id"], "result": {"tools": TOOLS}})
    elif method == "tools/call":
        params = request.get("params", {})
        name = params.get("name")
        args = params.get("arguments") or {}
        log_tool(name, args)
        if name == "list_projects":
            if mode == "global-list-hang":
                time.sleep(60)
            tool_result(request["id"], json.dumps({"projects": [{"name": "unrelated", "path": "/tmp/unrelated"}]}))
        elif name == "index_status":
            project = args.get("project", "")
            if mode == "discovery-hang":
                time.sleep(60)
            elif mode == "discovery-malformed":
                tool_result(request["id"], "not-json")
            elif mode == "discovery-error":
                tool_result(request["id"], json.dumps({"status": "backend_unavailable", "message": "project not found while backend unavailable"}), True)
            elif mode == "discovery-mismatched-missing":
                tool_result(request["id"], json.dumps({"project": "different-project", "status": "missing"}), True)
            else:
                actual, binding = indexed_binding(project)
                if binding:
                    confirmation = {"project": actual, "status": "ready", "root_path": binding["repo_path"]}
                    if mode == "confirmation-missing-identity":
                        del confirmation["project"]
                    elif mode == "confirmation-malformed-identity":
                        confirmation["project"] = []
                    elif mode in ("confirmation-mismatched-identity", "index-wrong-project"):
                        confirmation["project"] = "different-provider-project"
                    elif mode == "confirmation-path-keyed-identity":
                        confirmation["project"] = binding["repo_path"]
                    elif mode == "index-missing-root":
                        del confirmation["root_path"]
                    elif mode == "index-malformed-root":
                        confirmation["root_path"] = []
                    elif mode == "index-wrong-root":
                        confirmation["root_path"] = os.path.join(binding["repo_path"], "stale-checkout")
                    elif mode == "index-unconfirmed-root":
                        confirmation["status"] = "indexed"
                    tool_result(request["id"], json.dumps(confirmation))
                elif mode == "missing-inventory":
                    tool_result(request["id"], json.dumps({"error": "project not found or not indexed", "hint": "Use list_projects to see all indexed projects", "available_projects": ["unrelated-inventory-project"], "count": 1}), True)
                elif mode in ("missing", "index-hang", "index-error", "index-error-secret", "index-malformed", "index-wrong-project", "index-missing-root", "index-malformed-root", "index-wrong-root", "index-unconfirmed-root", "confirmation-missing-identity", "confirmation-malformed-identity", "confirmation-mismatched-identity", "confirmation-path-keyed-identity", "background-budget-success", "background-budget-timeout"):
                    tool_result(request["id"], json.dumps({"project": project, "status": "missing"}), True)
                elif mode == "stale":
                    tool_result(request["id"], json.dumps({"project": project, "status": "stale"}))
                else:
                    tool_result(request["id"], json.dumps({"project": project, "status": "fresh"}))
        elif name == "index_repository":
            repo_path = args.get("repo_path", "")
            project = args.get("name", "")
            if not isinstance(repo_path, str) or not repo_path or not isinstance(project, str) or not project:
                tool_result(request["id"], "index_repository requires repo_path and stable name", True)
                continue
            if mode == "background-budget-success":
                # Keep this above process-startup jitter so the success-budget
                # test observes readiness rather than a completed background run.
                time.sleep(0.30)
            elif mode == "background-budget-timeout":
                time.sleep(0.30)
            if mode == "index-hang":
                time.sleep(60)
            if mode == "index-error":
                tool_result(request["id"], "index failed", True)
            elif mode == "index-error-secret":
                tool_result(request["id"], "Authorization: Bearer SECRET", True)
            elif mode == "index-malformed":
                tool_result(request["id"], "not-json SECRET")
            else:
                actual = stable_upsert(project, repo_path)
                tool_result(request["id"], json.dumps({"project": actual, "status": "indexed"}))
        elif name == "get_code_snippet" and mode == "cold-warm":
            snippet = bound_snippet(args.get("project", ""))
            if snippet is None:
                tool_result(request["id"], "bound source unavailable", True)
            else:
                tool_result(request["id"], json.dumps(snippet))
        else:
            if mode == "background-budget-success":
                time.sleep(0.05)
            elif mode == "background-budget-timeout":
                time.sleep(0.20)
            elif mode == "graph-timeout":
                time.sleep(60)
            if mode == "graph-closure":
                tool_result(request["id"], "exploration_closed", True)
            elif mode == "graph-systemic":
                tool_result(request["id"], "provider protocol is unusable SECRET", True)
            elif mode in ("active-root-handoff", "active-root-overlap-handoff") and name == "search_graph":
                active = args.get("query") == "active routing implementation"
                worker_rank = 7 if active else 5
                prefix = "active" if active else "sibling"
                results = []
                result_count = 10 if mode == "active-root-overlap-handoff" else 70
                for rank in range(1, result_count + 1):
                    if mode == "active-root-overlap-handoff":
                        if rank == 1:
                            symbol = "affinity_topic"
                            qualified_name = "temper-v1-production.src.model.affinity_topic"
                        elif active and 2 <= rank <= 6:
                            symbol = f"test_affinity_{rank}"
                            qualified_name = f"temper-v1-production.tests.route.{symbol}"
                        elif 2 <= rank <= 6:
                            results.append({"label": "Module", "file_path": f"src/{prefix}_{rank}.rs"})
                            continue
                        elif rank == 7:
                            symbol = "worker_for"
                            qualified_name = "temper-v1-production.src.route.worker_for"
                        elif rank == 8:
                            symbol = "worker_slot"
                            qualified_name = "temper-v1-production.src.route.worker_slot"
                        else:
                            symbol = f"active_{rank}"
                            qualified_name = f"temper-v1-production.src.route.{symbol}"
                        result = {
                            "name": symbol,
                            "qualified_name": qualified_name,
                            "label": "Function",
                            "file_path": "tests/route.rs" if 2 <= rank <= 6 else "src/route.rs",
                        }
                        if active and 2 <= rank <= 6:
                            result["is_test"] = True
                        results.append(result)
                    elif rank < worker_rank:
                        results.append({"label": "Module", "file_path": f"src/{prefix}_{rank}.rs"})
                    else:
                        symbol = (
                            "worker_slot" if active and rank == worker_rank
                            else "worker_slot" if mode == "active-root-overlap-handoff" and rank == worker_rank
                            else "sibling_worker_slot" if rank == worker_rank
                            else f"{prefix}_{rank}"
                        )
                        qualified_name = symbol
                        if mode == "active-root-overlap-handoff" and rank == worker_rank:
                            qualified_name = f"temper-v1-production.src.route.{symbol}"
                        result = {
                            "name": symbol,
                            "qualified_name": qualified_name,
                            "label": "Function",
                            "file_path": "src/route.rs",
                        }
                        results.append(result)
                payload = {"total": result_count, "has_more": False, "results": results}
                tool_result(request["id"], json.dumps(payload), structured=payload)
            elif mode == "active-root-overlap-handoff" and name == "trace_path":
                symbol = args.get("function_name")
                if symbol in ("affinity_topic", "worker_slot"):
                    callers = [
                        {
                            "name": "worker_slot",
                            "qualified_name": "temper-v1-production.src.route.worker_slot",
                        },
                        {
                            "name": "worker_for",
                            "qualified_name": "temper-v1-production.src.route.worker_for",
                        },
                    ] if symbol == "affinity_topic" else [
                        {
                            "name": "worker_for",
                            "qualified_name": "temper-v1-production.src.route.worker_for",
                        },
                        {
                            "name": "affinity_topic",
                            "qualified_name": "temper-v1-production.src.model.affinity_topic",
                        },
                    ]
                    payload = {
                        "function": {
                            "name": symbol,
                            "qualified_name": f"temper-v1-production.src.{'model' if symbol == 'affinity_topic' else 'route'}.{symbol}",
                        },
                        "callers": callers,
                    }
                    tool_result(request["id"], json.dumps(payload), structured=payload)
                else:
                    tool_result(request["id"], "invalid argument", True)
            elif mode in ("active-root-handoff", "active-root-overlap-handoff") and name == "get_code_snippet":
                symbol = args.get("qualified_name")
                expected = (
                    "temper-v1-production.src.model.affinity_topic",
                    "temper-v1-production.src.route.worker_slot",
                    "temper-v1-production.src.route.worker_for",
                    "temper-v1-production.tests.route.test_affinity_2",
                    "temper-v1-production.tests.route.test_affinity_3",
                    "temper-v1-production.tests.route.test_affinity_4",
                    "temper-v1-production.tests.route.test_affinity_5",
                    "temper-v1-production.tests.route.test_affinity_6",
                    "test_affinity_2",
                    "test_affinity_3",
                    "test_affinity_4",
                    "test_affinity_5",
                    "test_affinity_6",
                ) if mode == "active-root-overlap-handoff" else ("worker_slot", "sibling_worker_slot")
                if symbol not in expected:
                    tool_result(request["id"], "invalid argument", True)
                else:
                    test_symbol = symbol.startswith("test_affinity_")
                    qualified_symbol = (
                        f"temper-v1-production.tests.route.{symbol}"
                        if test_symbol
                        else symbol
                    )
                    file_path = (
                        "src/model.rs"
                        if symbol == "temper-v1-production.src.model.affinity_topic"
                        else "tests/route.rs"
                        if test_symbol or ".tests." in symbol
                        else "src/route.rs"
                    )
                    payload = local_snippet(symbol, file_path, qualified_symbol)
                    if test_symbol or ".tests." in symbol:
                        payload["is_test"] = True
                    tool_result(request["id"], json.dumps(payload), structured=payload)
            elif mode == "graph-errors" and args.get("query") == "invalid":
                tool_result(request["id"], "invalid argument: query-local SECRET", True)
            elif mode == "graph-errors" and args.get("query") == "systemic":
                tool_result(request["id"], "unusable provider state Authorization: Bearer SECRET", True)
            elif mode == "graph-errors" and args.get("query") == "empty":
                payload = {"results": [], "total": 0, "has_more": False}
                tool_result(request["id"], json.dumps(payload), structured=payload)
            elif mode == "lineage-cases":
                target = {"name": "run", "qualified_name": "crate::engine::run", "file_path": "src/lib.rs"}
                if args.get("query") == "start":
                    payload = {"results": [target], "total": 1, "has_more": False}
                elif args.get("function_name") == "run":
                    payload = {"function": target, "callers": []}
                elif args.get("qualified_name") == "crate::engine::run":
                    payload = local_snippet("crate::engine::run", "src/lib.rs")
                elif args.get("query") == "duplicate":
                    payload = {"results": [target, target], "total": 2, "has_more": False}
                else:
                    payload = {"results": [], "total": 0, "has_more": False}
                tool_result(request["id"], json.dumps(payload), structured=payload)
            elif mode == "typed-lineage-parts":
                targets = [
                    {"name": leaf, "qualified_name": f"crate::engine::{leaf}", "file_path": "src/lib.rs", "is_test": leaf == "behavior"}
                    for leaf in ("run", "behavior", "caller", "source")
                ]
                if name == "get_code_snippet":
                    payload = local_snippet(args["qualified_name"], "src/lib.rs")
                    if args["qualified_name"].endswith("::behavior"):
                        payload["is_test"] = True
                elif name == "trace_path":
                    payload = {"function": targets[0], "callers": targets[2:]}
                else:
                    payload = {"results": targets, "total": len(targets), "has_more": False,
                               "docstring": "PRIVATE-TYPED-PART-SENTINEL"}
                tool_result(request["id"], json.dumps(payload), structured=payload)
            else:
                if name == "get_architecture":
                    payload = "PROVIDER-RESULT-SENTINEL" if mode == "anchor-cases" else "x" * 20000
                    tool_result(request["id"], payload)
                else:
                    if mode == "anchor-cases" and args.get("query") in ("large", "oversized"):
                        target = local_snippet(args["query"], f"src/{args['query']}.rs")
                    else:
                        target = {"name": "PROVIDER-RESULT-SENTINEL" if mode == "anchor-cases" else "FixtureMatch",
                                  "qualified_name": "fixture.src.lib.FixtureMatch", "file_path": "src/lib.rs"}
                    payload = {"results": [target], "total": 1, "has_more": False, "project": args.get("project", args.get("repo"))}
                    # Exercise presentation truncation with one bounded typed
                    # representation; mirroring 15 KB would exceed the separate
                    # aggregate typed-parts limit before source verification.
                    structured = None if mode == "anchor-cases" and args.get("query") == "large" else payload
                    tool_result(request["id"], json.dumps(payload), structured=structured)
    else:
        send({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32601, "message": "unknown method"}})
