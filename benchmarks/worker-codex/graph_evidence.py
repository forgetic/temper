"""Graph eligibility is independent of whether an agent solved the coding task."""

import hashlib
import json
from pathlib import Path


READ_TOOLS = {"search_graph", "query_graph", "trace_path", "get_code_snippet", "get_architecture",
              "search_code", "check_index_coverage", "index_status", "manage_adr"}


def result_payload(result, *, allow_error=False):
    if not isinstance(result, dict) or (result.get("isError") is True and not allow_error):
        raise ValueError("missing or unsuccessful MCP result")
    body = result.get("structured_content", result.get("structuredContent"))
    if body is None:
        texts = [part["text"] for part in result.get("content", []) if part.get("type") == "text"]
        if len(texts) != 1:
            raise ValueError("one structured MCP result is required")
        body = json.loads(texts[0])
    if not isinstance(body, dict):
        raise ValueError("MCP result must be an object")
    return body


def codex_graph_evidence(events_path, checkout, namespace_proof):
    """Require the successful current-root index before any scoped graph request."""
    errors, violations, calls = [], [], {}
    project, reads, indexes = None, 0, 0
    proof = namespace_proof if isinstance(namespace_proof, dict) else {}
    namespace = proof.get("namespace")
    instruction = proof.get("instructions")
    if (proof.get("complete") is not True or proof.get("missing_before_start") is not True
            or proof.get("source") != "host_index_status_before_codex"
            or not isinstance(namespace, str) or not namespace
            or proof.get("status_request") != {"name": "index_status", "arguments": {"project": namespace}}
            or proof.get("status_is_error") is not True
            or proof.get("status_error") != "project not found or not indexed"
            or not isinstance(instruction, str)
            or proof.get("instructions_sha256") != hashlib.sha256(instruction.encode()).hexdigest()):
        errors.append("pre_attempt_namespace_proof_incomplete")
    root = Path(checkout).resolve()
    try:
        rows = Path(events_path).read_text().splitlines()
    except OSError:
        rows = []
        errors.append("events_unavailable")
    for number, line in enumerate(rows, 1):
        try:
            event = json.loads(line)["event"]
            kind = event["type"]
            if kind == "capture.invalid_json":
                raise ValueError("invalid captured JSON")
            item = event.get("item", {})
            if item.get("type") != "mcp_tool_call" or item.get("server") != "codebase-memory-mcp":
                continue
            if kind not in {"item.started", "item.updated", "item.completed"}:
                raise ValueError("unknown MCP item lifecycle")
            identifier, tool, args = item["id"], item["tool"], item["arguments"]
            if not isinstance(identifier, str) or not isinstance(args, dict):
                raise ValueError("invalid MCP call identity or arguments")
            if kind == "item.started":
                if identifier in calls:
                    raise ValueError("duplicate MCP call")
                calls[identifier] = {"tool": tool, "args": args, "complete": False}
                if tool == "index_repository":
                    path = args.get("repo_path")
                    if not isinstance(path, str) or not path or (root / path).resolve() != root:
                        violations.append("index_outside_current_checkout")
                    if namespace is not None and args.get("name") != namespace:
                        violations.append("index_outside_reserved_namespace")
                elif tool != "list_projects":
                    if tool not in READ_TOOLS:
                        violations.append("unsupported_graph_tool")
                    if project is None:
                        violations.append("graph_read_before_current_index")
                    elif args.get("project") != project:
                        violations.append("graph_read_outside_current_project")
                    reads += 1
            else:
                call = calls.get(identifier)
                if call is None or call["complete"] or call["tool"] != tool or call["args"] != args:
                    raise ValueError("unpaired or inconsistent MCP item")
                if kind != "item.completed":
                    continue
                call["complete"] = True
                if tool == "index_repository":
                    if item.get("status") != "completed" or item.get("error") is not None:
                        continue
                    body = result_payload(item.get("result"))
                    indexed = body.get("project")
                    if body.get("status") != "indexed" or not isinstance(indexed, str) or not indexed:
                        raise ValueError("index completion does not identify a ready project")
                    if namespace is not None and indexed != namespace:
                        violations.append("index_outside_reserved_namespace")
                    if project is not None and project != indexed:
                        violations.append("index_changes_project_namespace")
                    project, indexes = indexed, indexes + 1
        except (ValueError, KeyError, TypeError, AttributeError):
            errors.append(f"malformed_graph_event:{number}")
    if not calls or not all(call["complete"] for call in calls.values()):
        errors.append("graph_call_capture_incomplete")
    if not indexes:
        violations.append("no_successful_current_index")
    if not reads:
        violations.append("no_scoped_graph_reads")
    return {"source": "codex_public_mcp_events_and_pre_attempt_namespace_proof",
            "complete": not errors, "eligible": not errors and not violations,
            "scope_valid": not violations and not any(
                error != "pre_attempt_namespace_proof_incomplete" for error in errors),
            "project": project, "successful_indexes": indexes, "scoped_reads": reads,
            "errors": sorted(set(errors)), "violations": sorted(set(violations))}


def native_graph_evidence(journal_root, expected_traces, expected_graph_calls):
    """Inspect every graph completion for the typed conventional-fallback signal."""
    paths = sorted(Path(journal_root).rglob("events.jsonl"))
    errors, fallbacks, calls = [], [], {}
    if not paths or len(paths) != expected_traces:
        errors.append("native_graph_trace_count_mismatch")
    for path in paths:
        try:
            rows = path.read_text().splitlines()
            if not rows:
                raise ValueError("empty native trace")
            for number, line in enumerate(rows, 1):
                row = json.loads(line)
                if type(row.get("seq")) is not int or row["seq"] != number:
                    raise ValueError("native trace sequence gap")
                event = row["event"]
                if event["type"] not in {"tool.started", "tool.finished"}:
                    continue
                data = event["data"]
                name = data["name"]
                if not name.startswith("codebase_memory_"):
                    continue
                key = (str(path), data["call_id"])
                if event["type"] == "tool.started":
                    if key in calls:
                        raise ValueError("duplicate native graph call")
                    calls[key] = {"name": name, "complete": False}
                    continue
                call = calls.get(key)
                if call is None or call["complete"] or call["name"] != name:
                    raise ValueError("unpaired native graph completion")
                call["complete"] = True
                failure = data.get("failure")
                if failure is not None:
                    fallback = failure["fallback_to_conventional_discovery"]
                    if type(fallback) is not bool:
                        raise ValueError("missing typed fallback signal")
                    if fallback:
                        fallbacks.append({"tool": name, "category": failure.get("category"),
                                          "reason": failure.get("reason"), "call_id": data["call_id"]})
                elif data.get("status") != "succeeded":
                    raise ValueError("unsuccessful graph call lacks failure metadata")
        except (OSError, ValueError, KeyError, TypeError, AttributeError):
            errors.append(f"malformed_native_graph_trace:{path.parent.name}")
    if (type(expected_graph_calls) is not int or expected_graph_calls <= 0
            or len(calls) != expected_graph_calls or not all(c["complete"] for c in calls.values())):
        errors.append("native_graph_call_capture_incomplete")
    return {"source": "native_tool_finished_failure_metadata", "complete": not errors,
            "eligible": not errors and not fallbacks, "observed_graph_calls": len(calls),
            "systemic_fallback_observed": bool(fallbacks), "fallbacks": fallbacks,
            "errors": sorted(set(errors))}
