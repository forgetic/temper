import json
import os
import sys
import time

LOG_PATH = sys.argv[1]
REPO_ROOT = sys.argv[2]
FIXTURE_PROJECT = sys.argv[3]
SAFE_TOOLS = set(json.loads(sys.argv[4]))
HIDDEN_TOOLS = set(json.loads(sys.argv[5]))
READINESS_DELAY_MS = int(sys.argv[6])
FORCED_FAILURE_TOOL = "" if sys.argv[7] == "-" else sys.argv[7]
FORCED_FAILURE_AFTER_CALLS = int(sys.argv[8])
LIFECYCLE_PROFILE = sys.argv[9]
STATE_PATH = LOG_PATH + ".state.json"
SEQUENTIAL_STAGE = 0
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
        "name": "search_code",
        "description": "Refine a graph-selected implementation symbol",
        "inputSchema": {
            "type": "object",
            "properties": {
                "pattern": {"type": "string"},
                "project": {"type": "string"},
            },
            "required": ["pattern"],
        },
    },
    {
        "name": "trace_path",
        "description": "Trace a refined symbol to its caller",
        "inputSchema": {
            "type": "object",
            "properties": {
                "function_name": {"type": "string"},
                "project": {"type": "string"},
            },
            "required": ["function_name"],
        },
    },
    {
        "name": "get_code_snippet",
        "description": "Read source from the bound project",
        "inputSchema": {
            "type": "object",
            "properties": {
                "project": {"type": "string"},
                "qualified_name": {"type": "string"},
            },
            "required": ["qualified_name"],
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


def log_tool(name, arguments, delay_ms=None, is_error=False, fixture_event=None):
    value = {
        "tool": name,
        "arguments": arguments,
        "fixture_project": FIXTURE_PROJECT,
        "is_error": is_error,
    }
    if delay_ms is not None:
        value["delay_ms"] = delay_ms
    if fixture_event is not None:
        value["fixture_event"] = fixture_event
    with open(LOG_PATH, "a", encoding="utf-8") as handle:
        handle.write(json.dumps(value, sort_keys=True) + "\n")


def load_state():
    try:
        with open(STATE_PATH, "r", encoding="utf-8") as handle:
            return json.load(handle)
    except (OSError, json.JSONDecodeError):
        return {"projects": {}, "counters": {"project_creations": 0, "rebinds": 0}}


def save_state(state):
    temporary = STATE_PATH + ".tmp"
    with open(temporary, "w", encoding="utf-8") as handle:
        json.dump(state, handle, sort_keys=True)
    os.replace(temporary, STATE_PATH)


def normalized_provider_project(project):
    return "normalized-" + project


def seed_fresh_prior_binding(project):
    state = load_state()
    if project not in state["projects"]:
        state["projects"][project] = {
            "repo_path": "retired-prepared-checkout",
            "binding": "prior_prepared_checkout",
        }
        state["counters"]["project_creations"] += 1
        save_state(state)


def rebind_current_root(project, repo_path):
    state = load_state()
    confirmed_project = normalized_provider_project(project)
    if project not in state["projects"]:
        state["counters"]["project_creations"] += 1
    else:
        # The production provider canonicalizes the requested stable key. It
        # remains one retained provider project, not a second path-keyed one.
        del state["projects"][project]
    state["projects"][confirmed_project] = {
        "requested_stable_project": project,
        "repo_path": repo_path,
        "binding": "current_prepared_checkout",
    }
    state["counters"]["rebinds"] += 1
    save_state(state)
    return confirmed_project


def current_root_source(project, relative_path):
    state = load_state()
    binding = state["projects"].get(project, {})
    if binding.get("binding") != "current_prepared_checkout":
        return None
    root = binding.get("repo_path")
    if not isinstance(root, str):
        return None
    source_path = os.path.abspath(os.path.join(root, relative_path))
    if os.path.commonpath([os.path.abspath(root), source_path]) != os.path.abspath(root):
        return None
    try:
        with open(source_path, "r", encoding="utf-8") as handle:
            return handle.read()
    except OSError:
        return None


def text_result(text, is_error=False):
    return {"content": [{"type": "text", "text": text}], "isError": is_error}


def has_current_root_profile():
    return LIFECYCLE_PROFILE in (
        "stable-rebind",
        "graph-consumption",
        "sequential-graph-evidence",
    )


def is_sequential_graph_evidence_profile():
    return LIFECYCLE_PROFILE == "sequential-graph-evidence"


def sequential_step(expected_stage, expected_argument, actual_argument):
    global SEQUENTIAL_STAGE
    if not is_sequential_graph_evidence_profile():
        return True
    if SEQUENTIAL_STAGE != expected_stage or actual_argument != expected_argument:
        return False
    SEQUENTIAL_STAGE += 1
    return True


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
            if has_current_root_profile():
                project = arguments.get("project", "")
                binding = load_state()["projects"].get(project)
                if binding and binding.get("binding") == "current_prepared_checkout":
                    log_tool(name, arguments, fixture_event="current_root_confirmed")
                    result = text_result(json.dumps({
                        "project": project,
                        "root_path": binding["repo_path"],
                        "status": "ready",
                    }))
                else:
                    seed_fresh_prior_binding(project)
                    log_tool(name, arguments, fixture_event="fresh_prior_binding")
                    result = text_result(json.dumps({"project": project, "status": "fresh"}))
            else:
                log_tool(name, arguments, is_error=True)
                result = text_result(
                    json.dumps({"project": arguments.get("project", ""), "status": "missing"}),
                    True,
                )
        elif name == "index_repository":
            time.sleep(READINESS_DELAY_MS / 1000)
            provider_project = arguments.get("name", "")
            if has_current_root_profile():
                provider_project = rebind_current_root(
                    provider_project, arguments.get("repo_path", "")
                )
                fixture_event = "normalized_current_root_upsert"
            else:
                fixture_event = None
            log_tool(name, arguments, delay_ms=READINESS_DELAY_MS, fixture_event=fixture_event)
            result = text_result(json.dumps({
                "project": provider_project,
                "status": "indexed",
            }))
        elif name == "get_code_snippet":
            project = arguments.get("project", "")
            qualified_name = arguments.get("qualified_name", "")
            source_paths = {
                "retry_worker_topic": "src/lib.rs",
                "retry_worker_topic_retry_affinity": "tests/retry_affinity.rs",
            }
            if is_sequential_graph_evidence_profile():
                source_paths = {
                    "delivery_worker_topic": "src/lib.rs",
                    "delivery_worker_topic_preserves_canonical_topic": "tests/retry_affinity.rs",
                }
            source_path = source_paths.get(qualified_name)
            expected_stage = {
                "delivery_worker_topic": 3,
                "delivery_worker_topic_preserves_canonical_topic": 4,
            }.get(qualified_name)
            sequence_valid = (
                expected_stage is None
                or sequential_step(expected_stage, qualified_name, qualified_name)
            )
            source = (
                current_root_source(project, source_path)
                if has_current_root_profile() and source_path is not None and sequence_valid
                else None
            )
            if source is None:
                log_tool(name, arguments, is_error=True)
                result = text_result("bound source unavailable", True)
            else:
                log_tool(name, arguments, fixture_event="served_current_root_source")
                payload = {
                    "qualified_name": qualified_name,
                    "file_path": source_path,
                    "source": source,
                    "binding": "current_prepared_checkout",
                }
                if is_sequential_graph_evidence_profile() and qualified_name == "delivery_worker_topic":
                    payload["next_target"] = {
                        "tool": "get_code_snippet",
                        "qualified_name": "delivery_worker_topic_preserves_canonical_topic",
                    }
                result = text_result(json.dumps(payload))
        elif name == "search_code":
            project = arguments.get("project", "")
            current_root = current_root_source(project, "src/lib.rs") is not None
            sequence_valid = sequential_step(
                1,
                "delivery_worker_topic",
                arguments.get("pattern", ""),
            )
            successful = current_root and sequence_valid
            log_tool(
                name,
                arguments,
                is_error=not successful,
                fixture_event="served_current_root_code_refinement" if successful else None,
            )
            result = (
                text_result(
                    "SEQUENTIAL_CODE_RESULT next=trace_path:function_name=delivery_worker_topic"
                    if is_sequential_graph_evidence_profile()
                    else "FAKE_MCP_CODE_RESULT symbol=retry_worker_topic"
                )
                if successful
                else text_result("bound source unavailable", True)
            )
        elif name == "trace_path":
            project = arguments.get("project", "")
            current_root = current_root_source(project, "src/lib.rs") is not None
            sequence_valid = sequential_step(
                2,
                "delivery_worker_topic",
                arguments.get("function_name", ""),
            )
            successful = current_root and sequence_valid
            log_tool(
                name,
                arguments,
                is_error=not successful,
                fixture_event="served_current_root_graph_trace" if successful else None,
            )
            result = (
                text_result(
                    "SEQUENTIAL_TRACE_RESULT caller=delivery_worker_route "
                    "next=get_code_snippet:qualified_name=delivery_worker_topic"
                    if is_sequential_graph_evidence_profile()
                    else "FAKE_MCP_TRACE_RESULT caller=retry_worker_topic "
                    "focused_test=tests/retry_affinity.rs"
                )
                if successful
                else text_result("bound source unavailable", True)
            )
        elif name == "search_graph":
            GRAPH_CALLS += 1
            project = arguments.get("project", "")
            current_root = current_root_source(project, "src/lib.rs") is not None
            sequence_valid = sequential_step(
                0,
                "canonical delivery worker selection",
                arguments.get("query", ""),
            )
            is_error = (
                not current_root
                or not sequence_valid
                or (
                    name == FORCED_FAILURE_TOOL
                    and GRAPH_CALLS > FORCED_FAILURE_AFTER_CALLS
                )
            )
            log_tool(
                name,
                arguments,
                is_error=is_error,
                fixture_event="served_current_root_graph" if current_root and not is_error else None,
            )
            if is_error:
                result = text_result(
                    "fixture backend unavailable Authorization: Bearer MCP-FIXTURE-SECRET",
                    True,
                )
            else:
                result = text_result(
                    "SEQUENTIAL_GRAPH_RESULT next=search_code:pattern=delivery_worker_topic"
                    if is_sequential_graph_evidence_profile()
                    else "FAKE_MCP_GRAPH_RESULT implementation=src/lib.rs::retry_worker_topic "
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
