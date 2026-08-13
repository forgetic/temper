import json
import os
import sys
import time
import uuid

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
RESULT_DRIVEN_STAGE = 0
TYPED_LINEAGE_STAGE = 0
MAPPED_GRAPH_STAGE = 0
RESULT_DRIVEN_TOKENS = {
    name: "opaque-" + uuid.uuid4().hex
    for name in ["root", "refinement", "trace", "implementation", "behavioral_test"]
}
TYPED_LINEAGE_TOKENS = {
    "root": "crate::fixture::anchor_" + uuid.uuid4().hex,
    "behavioral_test": "crate::fixture::anchor_" + uuid.uuid4().hex + "_behavior",
}
MAPPED_GRAPH_TOKENS = {
    "implementation": "crate::fixture::routing_" + uuid.uuid4().hex + "::worker_slot",
    "caller": "crate::fixture::delivery_" + uuid.uuid4().hex + "::DeliveryAttempt",
    "source": "crate::fixture::delivery_" + uuid.uuid4().hex + "::worker_for",
    "unavailable": "crate::fixture::unavailable_" + uuid.uuid4().hex,
}
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


def ensure_result_driven_tokens():
    if not is_result_driven_guidance_profile():
        return
    state = load_state()
    state["decision_tokens"] = RESULT_DRIVEN_TOKENS
    save_state(state)


def ensure_typed_lineage_tokens():
    if not is_typed_lineage_profile():
        return
    state = load_state()
    state["typed_lineage_tokens"] = TYPED_LINEAGE_TOKENS
    save_state(state)


def result_driven_step(expected_stage, expected_token, actual_token):
    global RESULT_DRIVEN_STAGE
    if not is_result_driven_guidance_profile():
        return True
    if RESULT_DRIVEN_STAGE != expected_stage or actual_token != RESULT_DRIVEN_TOKENS[expected_token]:
        return False
    RESULT_DRIVEN_STAGE += 1
    return True


def typed_lineage_step(expected_stage, expected_value, actual_value):
    global TYPED_LINEAGE_STAGE
    if not is_typed_lineage_profile():
        return True
    if TYPED_LINEAGE_STAGE != expected_stage or actual_value != expected_value:
        return False
    TYPED_LINEAGE_STAGE += 1
    return True


def terminal_function_name(qualified_name):
    return qualified_name.rsplit("::", 1)[-1]


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


def text_result(text, is_error=False, structured=None):
    result = {"content": [{"type": "text", "text": text}], "isError": is_error}
    if structured is not None:
        result["structuredContent"] = structured
    return result


def has_current_root_profile():
    return LIFECYCLE_PROFILE in (
        "stable-rebind",
        "graph-consumption",
        "sequential-graph-evidence",
        "result-driven-decision-guidance",
        "provider-result-anchor",
        "provider-neutral-anchor-lineage",
        "mapped-live-graph-consumption",
    )


def is_result_driven_guidance_profile():
    return LIFECYCLE_PROFILE in ("result-driven-decision-guidance", "provider-result-anchor")


def is_typed_lineage_profile():
    return LIFECYCLE_PROFILE == "provider-neutral-anchor-lineage"


def is_mapped_graph_profile():
    return LIFECYCLE_PROFILE == "mapped-live-graph-consumption"


def ensure_mapped_graph_tokens():
    if not is_mapped_graph_profile():
        return
    state = load_state()
    state["mapped_graph_tokens"] = MAPPED_GRAPH_TOKENS
    save_state(state)


def mapped_graph_step(expected_stage, expected_value, actual_value):
    global MAPPED_GRAPH_STAGE
    if not is_mapped_graph_profile():
        return True
    if MAPPED_GRAPH_STAGE != expected_stage or actual_value != expected_value:
        return False
    MAPPED_GRAPH_STAGE += 1
    return True


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
        ensure_result_driven_tokens()
        ensure_typed_lineage_tokens()
        ensure_mapped_graph_tokens()
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
            if is_mapped_graph_profile():
                source_stage = {
                    MAPPED_GRAPH_TOKENS["caller"]: (3, "src/lib.rs"),
                    MAPPED_GRAPH_TOKENS["source"]: (4, "tests/dispatch_behavior.rs"),
                    MAPPED_GRAPH_TOKENS["unavailable"]: (5, None),
                }.get(qualified_name)
                source = (
                    current_root_source(project, source_stage[1])
                    if source_stage is not None
                    and source_stage[1] is not None
                    and mapped_graph_step(source_stage[0], qualified_name, qualified_name)
                    else None
                )
                unavailable = (
                    source_stage is not None
                    and source_stage[0] == 5
                    and mapped_graph_step(source_stage[0], qualified_name, qualified_name)
                )
                if source is None:
                    log_tool(
                        name,
                        arguments,
                        is_error=True,
                        fixture_event="served_mapped_unavailable" if unavailable else None,
                    )
                    result = text_result("bound source unavailable", True)
                else:
                    if source_stage[0] == 3:
                        payload = {
                            "name": terminal_function_name(qualified_name),
                            "qualified_name": qualified_name,
                            "file_path": source_stage[1],
                            "source": source,
                            "binding": "current_prepared_checkout",
                            "source_metadata": {
                                "related_source_references": [
                                    {"qualifiedName": MAPPED_GRAPH_TOKENS["source"]}
                                ]
                            },
                        }
                    else:
                        payload = {
                            "name": terminal_function_name(qualified_name),
                            "qualified_name": qualified_name,
                            "file_path": source_stage[1],
                            "source": source,
                            "binding": "current_prepared_checkout",
                            "source_metadata": {
                                "kind": "focused_test",
                                "next_target": {
                                    "qualifiedName": MAPPED_GRAPH_TOKENS["unavailable"]
                                },
                            },
                        }
                    log_tool(
                        name,
                        arguments,
                        fixture_event="served_mapped_current_root_source",
                    )
                    result = text_result(json.dumps(payload), structured=payload)
                send({"jsonrpc": "2.0", "id": request["id"], "result": result})
                continue
            if is_typed_lineage_profile():
                source_stage = {
                    TYPED_LINEAGE_TOKENS["root"]: (2, "src/lib.rs"),
                    TYPED_LINEAGE_TOKENS["behavioral_test"]: (3, "tests/dispatch_behavior.rs"),
                }.get(qualified_name)
                source = (
                    current_root_source(project, source_stage[1])
                    if source_stage is not None
                    and typed_lineage_step(source_stage[0], qualified_name, qualified_name)
                    else None
                )
                if source is None:
                    log_tool(name, arguments, is_error=True)
                    result = text_result("bound source unavailable", True)
                else:
                    payload = {
                        "source": source,
                        "binding": "current_prepared_checkout",
                    }
                    if source_stage[0] == 2:
                        payload.update({
                            "qualifiedName": qualified_name,
                            "implementation_source": qualified_name,
                            "next": TYPED_LINEAGE_TOKENS["behavioral_test"],
                            "next_target": {
                                "qualifiedName": TYPED_LINEAGE_TOKENS["behavioral_test"],
                            },
                        })
                    else:
                        payload.update({
                            "functionName": terminal_function_name(qualified_name),
                            "behavioral_test": terminal_function_name(qualified_name),
                        })
                    log_tool(name, arguments, fixture_event="served_typed_lineage_consumer")
                    result = text_result(json.dumps(payload))
                send({"jsonrpc": "2.0", "id": request["id"], "result": result})
                continue
            if is_result_driven_guidance_profile():
                source_stage = {
                    RESULT_DRIVEN_TOKENS["implementation"]: (3, "implementation", "src/lib.rs"),
                    RESULT_DRIVEN_TOKENS["behavioral_test"]: (4, "behavioral_test", "tests/dispatch_behavior.rs"),
                }.get(qualified_name)
                source = (
                    current_root_source(project, source_stage[2])
                    if source_stage is not None
                    and result_driven_step(source_stage[0], source_stage[1], qualified_name)
                    else None
                )
                if source is None:
                    log_tool(name, arguments, is_error=True)
                    result = text_result("bound source unavailable", True)
                else:
                    payload = {
                        "qualified_name": qualified_name,
                        "file_path": source_stage[2],
                        "source": source,
                        "binding": "current_prepared_checkout",
                    }
                    if source_stage[1] == "implementation":
                        payload["next"] = RESULT_DRIVEN_TOKENS["behavioral_test"]
                        payload["implementation_source"] = RESULT_DRIVEN_TOKENS["implementation"]
                    else:
                        payload["behavioral_test"] = RESULT_DRIVEN_TOKENS["behavioral_test"]
                    log_tool(name, arguments, fixture_event="served_result_derived_consumer")
                    result = text_result(json.dumps(payload))
                send({"jsonrpc": "2.0", "id": request["id"], "result": result})
                continue
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
            if is_mapped_graph_profile():
                project = arguments.get("project", "")
                expected = terminal_function_name(MAPPED_GRAPH_TOKENS["implementation"])
                successful = (
                    current_root_source(project, "src/lib.rs") is not None
                    and mapped_graph_step(1, expected, arguments.get("pattern", ""))
                )
                payload = {
                    "results": [{
                        "name": expected,
                        "qualified_name": MAPPED_GRAPH_TOKENS["implementation"],
                        "file_path": "src/lib.rs",
                        "related_source_references": [{
                            "qualifiedName": MAPPED_GRAPH_TOKENS["caller"],
                        }],
                    }]
                }
                log_tool(
                    name,
                    arguments,
                    is_error=not successful,
                    fixture_event="served_mapped_carry_forward" if successful else None,
                )
                result = (
                    text_result(json.dumps(payload), structured=payload)
                    if successful
                    else text_result("bound source unavailable", True)
                )
                send({"jsonrpc": "2.0", "id": request["id"], "result": result})
                continue
            if is_result_driven_guidance_profile():
                project = arguments.get("project", "")
                successful = (
                    current_root_source(project, "src/lib.rs") is not None
                    and result_driven_step(1, "refinement", arguments.get("pattern", ""))
                )
                log_tool(
                    name,
                    arguments,
                    is_error=not successful,
                    fixture_event="served_result_derived_consumer" if successful else None,
                )
                result = (
                    text_result(json.dumps({
                        "marker": "RESULT_DRIVEN_CODE_RESULT",
                        "next": RESULT_DRIVEN_TOKENS["trace"],
                    }))
                    if successful
                    else text_result("bound source unavailable", True)
                )
                send({"jsonrpc": "2.0", "id": request["id"], "result": result})
                continue
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
            if is_mapped_graph_profile():
                project = arguments.get("project", "")
                expected = terminal_function_name(MAPPED_GRAPH_TOKENS["implementation"])
                successful = (
                    current_root_source(project, "src/lib.rs") is not None
                    and mapped_graph_step(2, expected, arguments.get("function_name", ""))
                )
                payload = {
                    "function": expected,
                    "callers": [{
                        "name": terminal_function_name(MAPPED_GRAPH_TOKENS["caller"]),
                        "qualified_name": MAPPED_GRAPH_TOKENS["caller"],
                    }],
                    "related_sources": [{
                        "qualifiedName": MAPPED_GRAPH_TOKENS["source"],
                    }],
                }
                log_tool(
                    name,
                    arguments,
                    is_error=not successful,
                    fixture_event="served_mapped_carry_forward" if successful else None,
                )
                result = (
                    text_result(json.dumps(payload), structured=payload)
                    if successful
                    else text_result("bound source unavailable", True)
                )
                send({"jsonrpc": "2.0", "id": request["id"], "result": result})
                continue
            if is_typed_lineage_profile():
                project = arguments.get("project", "")
                expected = terminal_function_name(TYPED_LINEAGE_TOKENS["root"])
                successful = (
                    current_root_source(project, "src/lib.rs") is not None
                    and typed_lineage_step(1, expected, arguments.get("function_name", ""))
                )
                log_tool(
                    name,
                    arguments,
                    is_error=not successful,
                    fixture_event="served_typed_lineage_consumer" if successful else None,
                )
                result = (
                    text_result(json.dumps({
                        "marker": "TYPED_LINEAGE_TRACE_RESULT",
                        "qualified_name": TYPED_LINEAGE_TOKENS["root"],
                        "caller_model": expected,
                    }))
                    if successful
                    else text_result("bound source unavailable", True)
                )
                send({"jsonrpc": "2.0", "id": request["id"], "result": result})
                continue
            if is_result_driven_guidance_profile():
                project = arguments.get("project", "")
                successful = (
                    current_root_source(project, "src/lib.rs") is not None
                    and result_driven_step(2, "trace", arguments.get("function_name", ""))
                )
                log_tool(
                    name,
                    arguments,
                    is_error=not successful,
                    fixture_event="served_result_derived_consumer" if successful else None,
                )
                result = (
                    text_result(json.dumps({
                        "marker": "RESULT_DRIVEN_TRACE_RESULT",
                        "next": RESULT_DRIVEN_TOKENS["implementation"],
                        "caller_model": RESULT_DRIVEN_TOKENS["trace"],
                    }))
                    if successful
                    else text_result("bound source unavailable", True)
                )
                send({"jsonrpc": "2.0", "id": request["id"], "result": result})
                continue
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
            if is_mapped_graph_profile():
                project = arguments.get("project", "")
                successful = (
                    current_root_source(project, "src/lib.rs") is not None
                    and mapped_graph_step(0, "worker affinity routing", arguments.get("query", ""))
                )
                payload = {
                    "results": [{
                        "results": [{
                            "name": terminal_function_name(MAPPED_GRAPH_TOKENS["implementation"]),
                            "qualifiedName": MAPPED_GRAPH_TOKENS["implementation"],
                            "file_path": "src/lib.rs",
                        }],
                        "callers": [{
                            "qualifiedName": MAPPED_GRAPH_TOKENS["caller"],
                        }],
                    }]
                }
                log_tool(
                    name,
                    arguments,
                    is_error=not successful,
                    fixture_event="served_mapped_root" if successful else None,
                )
                result = (
                    text_result(json.dumps(payload), structured=payload)
                    if successful
                    else text_result("bound source unavailable", True)
                )
                send({"jsonrpc": "2.0", "id": request["id"], "result": result})
                continue
            if is_typed_lineage_profile():
                project = arguments.get("project", "")
                successful = (
                    current_root_source(project, "src/lib.rs") is not None
                    and typed_lineage_step(0, TYPED_LINEAGE_TOKENS["root"], TYPED_LINEAGE_TOKENS["root"])
                )
                log_tool(
                    name,
                    arguments,
                    is_error=not successful,
                    fixture_event="served_typed_lineage_producer" if successful else None,
                )
                result = (
                    text_result(json.dumps({
                        "marker": "TYPED_LINEAGE_GRAPH_RESULT",
                        "current_root": TYPED_LINEAGE_TOKENS["root"],
                        "qualifiedName": TYPED_LINEAGE_TOKENS["root"],
                    }))
                    if successful
                    else text_result("bound source unavailable", True)
                )
                send({"jsonrpc": "2.0", "id": request["id"], "result": result})
                continue
            if is_result_driven_guidance_profile():
                project = arguments.get("project", "")
                successful = (
                    current_root_source(project, "src/lib.rs") is not None
                    and result_driven_step(0, "root", RESULT_DRIVEN_TOKENS["root"])
                )
                log_tool(
                    name,
                    arguments,
                    is_error=not successful,
                    fixture_event="served_result_driven_producer" if successful else None,
                )
                result = (
                    text_result(json.dumps({
                        "marker": "RESULT_DRIVEN_GRAPH_RESULT",
                        "current_root": RESULT_DRIVEN_TOKENS["root"],
                        "next": RESULT_DRIVEN_TOKENS["refinement"],
                    }))
                    if successful
                    else text_result("bound source unavailable", True)
                )
                send({"jsonrpc": "2.0", "id": request["id"], "result": result})
                continue
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
