#!/usr/bin/env python3
"""Hermetic MCP fixture for the controlled codebase-memory benchmark."""

import json
import os
import sys

MODE = sys.argv[1] if len(sys.argv) > 1 else "enabled"

# This is deliberately distinct from Temper's opaque, stable upsert key. The
# fixture has one repository, so its production-shaped normalized identity is
# fixed and can be asserted by the controlled harness.
NORMALIZED_PROJECT = "temper-benchmark-codebase-memory-routing-repair"


def graph_project(arguments):
    return arguments.get("repo", arguments.get("project", ""))


def state_path_from_args():
    try:
        index = sys.argv.index("--state")
        return sys.argv[index + 1]
    except (ValueError, IndexError):
        return None


STATE_PATH = state_path_from_args()

INDEX_PROPERTIES = {
    "repo_path": {"type": "string"},
    "name": {"type": "string"},
}
TOOLS = [
    {
        "name": "search_code",
        "description": "Search indexed source and return containing symbols",
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
        "name": "search_graph",
        "description": "Search indexed symbols and relationships",
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "repo": {"type": "string"},
            },
        },
    },
    {
        "name": "trace_path",
        "description": "Trace a targeted symbol to its callers",
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
        "description": "Read a targeted symbol from the confirmed current root",
        "inputSchema": {
            "type": "object",
            "properties": {
                "qualified_name": {"type": "string"},
                "repo": {"type": "string"},
            },
            "required": ["qualified_name"],
        },
    },
    {
        "name": "get_architecture",
        "description": "Summarize indexed architecture",
        "inputSchema": {
            "type": "object",
            "properties": {"project": {"type": "string"}},
        },
    },
    {
        "name": "list_projects",
        "description": "List indexed projects",
        "inputSchema": {"type": "object", "properties": {}},
    },
    {
        "name": "index_status",
        "description": "Return targeted index status",
        "inputSchema": {
            "type": "object",
            "properties": {"project": {"type": "string"}},
            "required": ["project"],
        },
    },
    {
        "name": "index_repository",
        "description": "Upsert a stable repository index",
        "inputSchema": {
            "type": "object",
            "properties": INDEX_PROPERTIES,
            "required": ["repo_path", "name"],
        },
    },
]


def load_state():
    if not STATE_PATH:
        return {"graph_reads": [], "projects": {}, "provider_invocations": 0}
    try:
        with open(STATE_PATH, encoding="utf-8") as handle:
            state = json.load(handle)
    except (FileNotFoundError, json.JSONDecodeError):
        state = {"graph_reads": [], "projects": {}, "provider_invocations": 0}
    state.setdefault("graph_reads", [])
    state.setdefault("projects", {})
    state.setdefault("provider_invocations", 0)
    return state


def save_state(state):
    if not STATE_PATH:
        return
    temporary = f"{STATE_PATH}.tmp.{os.getpid()}"
    with open(temporary, "w", encoding="utf-8") as handle:
        json.dump(state, handle, sort_keys=True)
    os.replace(temporary, STATE_PATH)


def send(value):
    sys.stdout.write(json.dumps(value, separators=(",", ":")) + "\n")
    sys.stdout.flush()


def tool_result(request_id, payload, is_error=False):
    send(
        {
            "jsonrpc": "2.0",
            "id": request_id,
            "result": {
                "content": [{"type": "text", "text": payload}],
                "isError": is_error,
            },
        }
    )


def confirmed_graph_read(project, tool):
    state = load_state()
    binding = state["projects"].get(project)
    # A requested stable key is retained for assertion only. It cannot be
    # used as a graph target: only the normalized identity returned by the
    # upsert and confirmed by the second targeted status request can serve a
    # graph read or current-root source snippet.
    if project != NORMALIZED_PROJECT or binding is None:
        return None
    state["provider_invocations"] += 1
    graph_read_number = state["provider_invocations"]
    state["graph_reads"].append(
        {
            "confirmed_project": project,
            "requested_stable_project": binding["requested_stable_project"],
            "tool": tool,
        }
    )
    save_state(state)
    identity = {
        "confirmed_project": project,
        "graph_read_project": project,
        "project_route": "confirmed_identity",
        "requested_stable_project": binding["requested_stable_project"],
        "provider_invocation": graph_read_number,
    }
    return identity, binding, graph_read_number


def current_root_source(project, qualified_name):
    snippet = {
        "DeliveryAttempt": ("src/model.rs", "DeliveryRouter::worker_for"),
        "DeliveryRouter::worker_for": ("src/delivery.rs", "repo/src/route.rs"),
        "alias_retries_stay_on_the_original_ordered_worker": (
            "tests/alias_retry.rs",
            "repo/tests/alias_retry.rs",
        ),
    }.get(qualified_name)
    graph_read = confirmed_graph_read(project, "get_code_snippet")
    if graph_read is None or snippet is None:
        return None
    identity, binding, _graph_read_number = graph_read
    relative_path, related_qualified_name = snippet
    try:
        with open(
            os.path.join(binding["root_path"], relative_path), encoding="utf-8"
        ) as handle:
            source = handle.read()
    except OSError:
        return None
    return identity | {
        "source_root": "confirmed_current_root",
        "source_path": relative_path,
        "qualified_name": qualified_name,
        "related_qualified_name": related_qualified_name,
        "source": source,
    }


for line in sys.stdin:
    if not line.strip():
        continue
    request = json.loads(line)
    if "id" not in request:
        continue
    request_id = request["id"]
    method = request.get("method")
    if method == "initialize":
        send(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "serverInfo": {
                        "name": "codebase-memory-mcp",
                        "version": "0.9.0",
                    },
                    "capabilities": {"tools": {}},
                },
            }
        )
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": request_id, "result": {"tools": TOOLS}})
    elif method == "tools/call":
        params = request.get("params") or {}
        name = params.get("name", "")
        arguments = params.get("arguments") or {}
        if name == "index_status":
            project = arguments.get("project", "")
            binding = load_state()["projects"].get(project)
            # The canonical root is intentionally withheld from the upsert and
            # exposed only by this targeted confirmation of the normalized ID.
            if project == NORMALIZED_PROJECT and binding is not None:
                tool_result(
                    request_id,
                    json.dumps(
                        {
                            "project": project,
                            "root_path": binding["root_path"],
                            "status": "ready",
                        },
                        sort_keys=True,
                    ),
                )
            elif MODE == "enabled":
                payload = {"project": project, "status": "missing"}
                tool_result(
                    request_id,
                    json.dumps(payload, sort_keys=True),
                    True,
                )
            else:
                tool_result(
                    request_id,
                    json.dumps({"project": project, "status": "fresh"}, sort_keys=True),
                )
        elif name == "list_projects":
            tool_result(
                request_id,
                json.dumps(
                    {
                        "projects": [
                            {
                                "name": "temper-benchmark/codebase-memory-routing-repair",
                                "path": "fixture/repo",
                            }
                        ]
                    },
                    sort_keys=True,
                ),
            )
        elif name == "index_repository":
            requested_stable_project = arguments.get("name", "")
            repo_path = os.path.realpath(arguments.get("repo_path", ""))
            state = load_state()
            state["projects"][NORMALIZED_PROJECT] = {
                "requested_stable_project": requested_stable_project,
                "root_path": repo_path,
            }
            save_state(state)
            tool_result(
                request_id,
                json.dumps(
                    {
                        "project": NORMALIZED_PROJECT,
                        "status": "indexed",
                    },
                    sort_keys=True,
                ),
            )
        elif MODE == "unavailable":
            # The wrapper must classify and replace this provider text. It must
            # never appear in model-visible output or retained safe diagnostics.
            tool_result(
                request_id,
                "fixture backend unavailable Authorization: Bearer MCP-FIXTURE-SECRET",
                True,
            )
        elif name == "search_graph":
            graph_read = confirmed_graph_read(graph_project(arguments), "search_graph")
            if graph_read is None:
                tool_result(request_id, "fixture stable project is not ready", True)
            else:
                identity, _binding, graph_read_number = graph_read
                query = arguments.get("query", "")
                if query == "alias retry behavioral regression":
                    results = [
                        {
                            "qualified_name": "alias_retries_stay_on_the_original_ordered_worker",
                            "file_path": "tests/alias_retry.rs",
                        }
                    ]
                else:
                    results = [
                        {
                            "qualified_name": "worker_slot",
                            "file_path": "src/route.rs",
                        },
                        {
                            "qualified_name": "DeliveryRouter::worker_for",
                            "file_path": "src/delivery.rs",
                        },
                    ]
                tool_result(
                    request_id,
                    json.dumps(
                        identity
                        | {
                            "readiness": (
                                "cold stable upsert is ready"
                                if graph_read_number == 1
                                else "warm stable project remains ready"
                            ),
                            "results": results,
                        },
                        sort_keys=True,
                    ),
                )
        elif name == "search_code":
            graph_read = confirmed_graph_read(arguments.get("project", ""), "search_code")
            if graph_read is None:
                tool_result(request_id, "fixture stable project is not ready", True)
            else:
                identity, _binding, graph_read_number = graph_read
                tool_result(
                    request_id,
                    json.dumps(
                        identity
                        | {
                            "readiness": (
                                "warm stable project remains ready"
                                if graph_read_number >= 2
                                else "cold stable upsert is ready"
                            ),
                            "results": [
                                {"qualified_name": "worker_slot", "file_path": "src/route.rs"},
                            ],
                        },
                        sort_keys=True,
                    ),
                )
        elif name == "trace_path":
            graph_read = confirmed_graph_read(arguments.get("project", ""), "trace_path")
            if graph_read is None or arguments.get("function_name") != "worker_slot":
                tool_result(request_id, "fixture stable symbol is not ready", True)
            else:
                identity, _binding, _graph_read_number = graph_read
                tool_result(
                    request_id,
                    json.dumps(
                        identity | {"callers": ["DeliveryRouter::worker_for"]},
                        sort_keys=True,
                    ),
                )
        elif name == "get_code_snippet":
            result = current_root_source(
                graph_project(arguments), arguments.get("qualified_name", "")
            )
            if result is None:
                tool_result(request_id, "fixture current-root source is not ready", True)
            else:
                tool_result(request_id, json.dumps(result, sort_keys=True))
        elif name == "get_architecture":
            graph_read = confirmed_graph_read(
                arguments.get("project", ""), "get_architecture"
            )
            if graph_read is None:
                tool_result(request_id, "fixture stable project is not ready", True)
            else:
                identity, _binding, _graph_read_number = graph_read
                tool_result(
                    request_id,
                    json.dumps(
                        identity | {"architecture": "delivery fixture summary"},
                        sort_keys=True,
                    ),
                )
        else:
            tool_result(
                request_id,
                "fixture does not expose this codebase-memory tool",
                True,
            )
    else:
        send(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "error": {"code": -32601, "message": "unknown method"},
            }
        )
