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
FORCED_FAILURE_TOOL = sys.argv[7]
FORCED_FAILURE_AFTER_CALLS = int(sys.argv[8])
LIFECYCLE_PROFILE = sys.argv[9]
STATE_PATH = LOG_PATH + ".state.json"
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
        "name": "get_code_snippet",
        "description": "Read source from the bound project",
        "inputSchema": {
            "type": "object",
            "properties": {
                "project": {"type": "string"},
                "path": {"type": "string"},
            },
            "required": ["path"],
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
            if LIFECYCLE_PROFILE == "stable-rebind":
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
            if LIFECYCLE_PROFILE == "stable-rebind":
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
            path = arguments.get("path", "")
            source = (
                current_root_source(project, path)
                if LIFECYCLE_PROFILE == "stable-rebind"
                else None
            )
            if source is None:
                log_tool(name, arguments, is_error=True)
                result = text_result("bound source unavailable", True)
            else:
                log_tool(name, arguments, fixture_event="served_current_root_source")
                result = text_result(json.dumps({
                    "result": "FAKE_MCP_SNIPPET_RESULT",
                    "path": path,
                    "source": source,
                    "binding": "current_prepared_checkout",
                }))
        elif name == "search_graph":
            GRAPH_CALLS += 1
            project = arguments.get("project", "")
            current_root = current_root_source(project, "src/lib.rs") is not None
            is_error = (
                not current_root
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
