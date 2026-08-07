#!/usr/bin/env python3
"""Deterministic persistent fake for codebase-memory lifecycle tests.

The provider keeps all state in a lock-protected JSON file so independent MCP
processes observe one cache.  Tests can seed projects, independently delay each
lifecycle operation, inject per-project failures, and inspect bounded counters
and request evidence without depending on process lifetime.
"""

import argparse
import contextlib
import fcntl
import json
import os
import sys
import tempfile
import time


def arguments():
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument("--state", required=True)
    parser.add_argument("--log", default="")
    parser.add_argument("--mode", default="stateful")
    parser.add_argument("--seed-json", default="{}")
    parser.add_argument("--safe-tools-json", default="[]")
    parser.add_argument("--hidden-tools-json", default="[]")
    parser.add_argument("--fail-tools-json", default="{}")
    parser.add_argument("--delay-list-ms", type=int, default=0)
    parser.add_argument("--delay-status-ms", type=int, default=0)
    parser.add_argument("--delay-index-ms", type=int, default=0)
    parser.add_argument("--delay-delete-ms", type=int, default=0)
    parser.add_argument("--evidence-limit", type=int, default=64)
    return parser.parse_args()


ARGS = arguments()
SEED = json.loads(ARGS.seed_json)
FAILURES = json.loads(ARGS.fail_tools_json)
SAFE_TOOLS = set(json.loads(ARGS.safe_tools_json))
HIDDEN_TOOLS = set(json.loads(ARGS.hidden_tools_json))
EVIDENCE_LIMIT = max(1, min(ARGS.evidence_limit, 256))

INDEX_PROPERTIES = {
    "repo_path": {"type": "string"},
    "name": {"type": "string"},
}
if ARGS.mode == "incompatible-schema":
    del INDEX_PROPERTIES["name"]

TOOLS = [
    {
        "name": "search_code",
        "description": "Search indexed code",
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
        "name": "get_architecture",
        "description": "Summarize architecture",
        "inputSchema": {
            "type": "object",
            "properties": {"project": {"type": "string"}},
        },
    },
    {
        "name": "list_projects",
        "description": "List a bounded page of indexed projects",
        "inputSchema": {
            "type": "object",
            "properties": {
                "limit": {"type": "integer"},
                "cursor": {"type": "string"},
            },
            "required": ["limit"],
        },
    },
    {
        "name": "index_status",
        "description": "Read one project status",
        "inputSchema": {
            "type": "object",
            "properties": {"project": {"type": "string"}},
            "required": ["project"],
        },
    },
    {
        "name": "detect_changes",
        "description": "Detect changes",
        "inputSchema": {
            "type": "object",
            "properties": {"project": {"type": "string"}},
        },
    },
    {
        "name": "index_repository",
        "description": "Stable repository upsert",
        "inputSchema": {
            "type": "object",
            "properties": INDEX_PROPERTIES,
            "required": ["repo_path"],
        },
    },
    {
        "name": "delete_project",
        "description": "Delete one provider project",
        "inputSchema": {
            "type": "object",
            "properties": {"project": {"type": "string"}},
            "required": ["project"],
        },
    },
    {"name": "manage_adr", "description": "Write ADRs", "inputSchema": {"type": "object", "properties": {}}},
    {"name": "ingest_traces", "description": "Ingest traces", "inputSchema": {"type": "object", "properties": {}}},
    {"name": "query_graph", "description": "Raw graph query", "inputSchema": {"type": "object", "properties": {}}},
]
if SAFE_TOOLS or HIDDEN_TOOLS:
    TOOLS = [tool for tool in TOOLS if tool["name"] in SAFE_TOOLS | HIDDEN_TOOLS]


def normalize_project(raw):
    record = dict(raw) if isinstance(raw, dict) else {"project": str(raw)}
    project = record.get("project") or record.get("name") or record.get("id")
    path = record.get("repo_path") or record.get("root_path") or record.get("path")
    if not project:
        project = path
    if not project:
        raise ValueError("seeded project requires project/name/id or a repository path")
    status = record.get("status") or record.get("state")
    if not status:
        status = "stale" if record.get("stale") else "fresh"
    estimated = record.get("estimated_bytes", record.get("size_bytes", 128))
    return {
        "project": str(project),
        "name": str(project),
        "repo_path": str(path or ""),
        "root_path": str(path or ""),
        "status": str(status),
        "stale": str(status).lower() == "stale",
        "updated_at_unix_secs": int(record.get("updated_at_unix_secs", 1)),
        "ownership": record.get("ownership"),
        "estimated_bytes": int(estimated) if estimated is not None else None,
        "indexing_active": record.get("indexing_active"),
    }


def initial_state():
    raw_projects = SEED.get("projects", []) if isinstance(SEED, dict) else []
    projects = {}
    for raw in raw_projects:
        record = normalize_project(raw)
        projects[record["project"]] = record
    return {
        "version": 1,
        "cache_instance_id": SEED.get("cache_instance_id", "fake-cache-v1"),
        "cache_bytes": SEED.get("cache_bytes"),
        "projects": projects,
        "counters": {},
        "evidence": [],
    }


def load_state():
    try:
        with open(ARGS.state, "r", encoding="utf-8") as source:
            state = json.load(source)
    except FileNotFoundError:
        state = initial_state()
    state.setdefault("projects", {})
    state.setdefault("counters", {})
    state.setdefault("evidence", [])
    state.setdefault("cache_instance_id", "fake-cache-v1")
    return state


def save_state(state):
    directory = os.path.dirname(os.path.abspath(ARGS.state))
    os.makedirs(directory, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(prefix=".fake-codebase-memory-", dir=directory)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as target:
            json.dump(state, target, sort_keys=True, separators=(",", ":"))
            target.write("\n")
            target.flush()
            os.fsync(target.fileno())
        os.replace(temporary, ARGS.state)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


@contextlib.contextmanager
def locked_state():
    lock_path = ARGS.state + ".lock"
    os.makedirs(os.path.dirname(os.path.abspath(lock_path)), exist_ok=True)
    with open(lock_path, "a+", encoding="utf-8") as lock:
        fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
        state = load_state()
        try:
            yield state
        finally:
            save_state(state)
            fcntl.flock(lock.fileno(), fcntl.LOCK_UN)


def append_log(value):
    if not ARGS.log:
        return
    os.makedirs(os.path.dirname(os.path.abspath(ARGS.log)), exist_ok=True)
    with open(ARGS.log, "a", encoding="utf-8") as target:
        target.write(json.dumps(value, sort_keys=True) + "\n")
        target.flush()


def record_request(name, args):
    evidence = {"name": name, "tool": name, "arguments": args, "pid": os.getpid()}
    with locked_state() as state:
        counters = state["counters"]
        counters[name] = int(counters.get(name, 0)) + 1
        state["evidence"].append(evidence)
        state["evidence"] = state["evidence"][-EVIDENCE_LIMIT:]
        append_log(evidence)


def mutate_state(operation):
    with locked_state() as state:
        return operation(state)


def cache_bytes(state):
    configured = state.get("cache_bytes")
    if configured is not None:
        return int(configured)
    total = 0
    for record in state["projects"].values():
        estimated = record.get("estimated_bytes")
        if estimated is not None:
            total += int(estimated)
    return total


def delay(name):
    milliseconds = {
        "list_projects": ARGS.delay_list_ms,
        "index_status": ARGS.delay_status_ms,
        "index_repository": ARGS.delay_index_ms,
        "delete_project": ARGS.delay_delete_ms,
    }.get(name, 0)
    if ARGS.mode == "global-list-hang" and name == "list_projects":
        milliseconds = 60_000
    if ARGS.mode == "discovery-hang" and name == "index_status":
        milliseconds = 60_000
    if ARGS.mode == "index-hang" and name == "index_repository":
        milliseconds = 60_000
    if milliseconds > 0:
        time.sleep(milliseconds / 1000.0)


def should_fail(name, project):
    selected = FAILURES.get(name, []) if isinstance(FAILURES, dict) else []
    if isinstance(selected, str):
        selected = [selected]
    return "*" in selected or project in selected


def send(value):
    sys.stdout.write(json.dumps(value, separators=(",", ":")) + "\n")
    sys.stdout.flush()


def result(request_id, payload, is_error=False):
    if not isinstance(payload, str):
        payload = json.dumps(payload, sort_keys=True, separators=(",", ":"))
    send({
        "jsonrpc": "2.0",
        "id": request_id,
        "result": {
            "content": [{"type": "text", "text": payload}],
            "isError": is_error,
        },
    })


def inventory(args):
    def select(state):
        records = [state["projects"][key] for key in sorted(state["projects"])]
        limit = max(1, min(int(args.get("limit", len(records) or 1)), 1000))
        try:
            offset = max(0, int(args.get("cursor", "0")))
        except (TypeError, ValueError):
            offset = 0
        end = min(len(records), offset + limit)
        return {
            "cache_instance_id": state["cache_instance_id"],
            "cache_bytes": cache_bytes(state),
            "projects": records[offset:end],
            "next_cursor": str(end) if end < len(records) else None,
        }
    return mutate_state(select)


def status(project):
    def lookup(state):
        record = state["projects"].get(project)
        if ARGS.mode == "missing":
            record = None
        if record is None:
            if ARGS.mode in ("normal", "global-list-hang"):
                return {"project": project, "status": "fresh", "cache_bytes": cache_bytes(state)}
            return {"project": project, "status": "missing", "cache_bytes": cache_bytes(state)}
        answer = dict(record)
        answer["project"] = project
        answer["cache_bytes"] = cache_bytes(state)
        if ARGS.mode == "stale":
            answer["status"] = "stale"
            answer["stale"] = True
        return answer
    return mutate_state(lookup)


def upsert(args):
    project = args.get("name")
    repo_path = args.get("repo_path")
    if not isinstance(project, str) or not project or not isinstance(repo_path, str) or not repo_path:
        return None

    def write(state):
        existing = state["projects"].get(project)
        if existing is None:
            state["counters"]["project_creations"] = int(
                state["counters"].get("project_creations", 0)
            ) + 1
        state["counters"]["upsert_writes"] = int(
            state["counters"].get("upsert_writes", 0)
        ) + 1
        estimated = existing.get("estimated_bytes", 128) if existing else 128
        updated = (
            existing.get("updated_at_unix_secs", 1_000_000)
            if existing
            else int(SEED.get("now_unix_secs", 1_000_000))
        )
        record = {
            "project": project,
            "name": project,
            "repo_path": repo_path,
            "root_path": repo_path,
            "status": "fresh",
            "stale": False,
            "updated_at_unix_secs": updated,
            "ownership": "temper",
            "estimated_bytes": estimated,
            "indexing_active": False,
        }
        state["projects"][project] = record
        return record

    return mutate_state(write)


def delete(project):
    def remove(state):
        existed = state["projects"].pop(project, None) is not None
        if existed:
            state["counters"]["project_deletions"] = int(
                state["counters"].get("project_deletions", 0)
            ) + 1
        return existed
    return mutate_state(remove)


if ARGS.mode == "hang":
    time.sleep(60)
    sys.exit(0)

# Materialize a seeded empty cache even if the process is killed during its
# first delayed request.
mutate_state(lambda _state: None)

for line in sys.stdin:
    if not line.strip():
        continue
    request = json.loads(line)
    if "id" not in request:
        continue
    method = request.get("method")
    if method == "initialize":
        record_request("initialize", request.get("params") or {})
        provider_name = "other-provider" if ARGS.mode == "incompatible-name" else "codebase-memory-mcp"
        provider_version = "0.8.1" if ARGS.mode == "incompatible-version" else "0.9.0"
        capabilities = {} if ARGS.mode == "incompatible-capability" else {"tools": {}}
        send({
            "jsonrpc": "2.0",
            "id": request["id"],
            "result": {
                "protocolVersion": "2024-11-05",
                "serverInfo": {"name": provider_name, "version": provider_version},
                "capabilities": capabilities,
            },
        })
    elif method == "tools/list":
        record_request("tools/list", request.get("params") or {})
        send({"jsonrpc": "2.0", "id": request["id"], "result": {"tools": TOOLS}})
    elif method == "tools/call":
        params = request.get("params") or {}
        name = params.get("name", "")
        args = params.get("arguments") or {}
        record_request(name, args)
        delay(name)
        if name == "list_projects":
            result(request["id"], inventory(args))
        elif name == "index_status":
            project = args.get("project", "")
            if ARGS.mode == "discovery-malformed":
                result(request["id"], "not-json")
            elif ARGS.mode == "discovery-error" or should_fail(name, project):
                result(request["id"], {"status": "backend_unavailable"}, True)
            else:
                result(request["id"], status(project))
        elif name == "index_repository":
            project = args.get("name", "")
            if ARGS.mode == "index-error" or should_fail(name, project):
                result(request["id"], {"status": "index_failed"}, True)
            else:
                record = upsert(args)
                if record is None:
                    result(request["id"], "index_repository requires repo_path and stable name", True)
                else:
                    result(request["id"], record)
        elif name == "delete_project":
            project = args.get("project", "")
            if should_fail(name, project):
                result(request["id"], {"status": "delete_failed"}, True)
            elif delete(project):
                result(request["id"], {"project": project, "status": "deleted"})
            else:
                result(request["id"], {"project": project, "status": "not_found"}, True)
        elif name == "search_code":
            project = args.get("project", "")
            if should_fail(name, project):
                result(request["id"], {"status": "search_failed"}, True)
            else:
                result(
                    request["id"],
                    "search_code result: FAKE_MCP_SEARCH_RESULT project={} query={}\n{}".format(
                        project, args.get("query", ""), "x" * 20000
                    ),
                )
        else:
            result(
                request["id"],
                "{} result for {}\n{}".format(name, json.dumps(args, sort_keys=True), "x" * 20000),
            )
    else:
        send({
            "jsonrpc": "2.0",
            "id": request["id"],
            "error": {"code": -32601, "message": "unknown method"},
        })
