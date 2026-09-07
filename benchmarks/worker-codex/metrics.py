"""Conservative metrics from retained public Codex events and Temper summaries."""

from __future__ import annotations

from collections import Counter
import json
import math
from pathlib import Path
import statistics


def codex_metrics(event_file: Path, process: dict) -> dict:
    calls = {}
    malformed = 0
    turn_completions = 0
    usage = Counter()
    usage_complete = True
    thread_id = None
    for line in event_file.read_text().splitlines():
        row = json.loads(line)
        event = row["event"]
        kind = event.get("type")
        if kind == "thread.started":
            thread_id = event.get("thread_id")
        elif kind == "capture.invalid_json":
            malformed += 1
        elif kind == "turn.completed":
            turn_completions += 1
            observed_usage = event.get("usage")
            if not isinstance(observed_usage, dict) or not all(
                    key in observed_usage for key in ["input_tokens", "cached_input_tokens", "output_tokens"]):
                usage_complete = False
            else:
                usage.update(observed_usage)
        elif kind in ("item.started", "item.updated", "item.completed"):
            item = event.get("item", {})
            name = _codex_tool_name(item)
            if name is None:
                continue
            call = calls.setdefault(item["id"], {"name": name, "completed": False})
            if kind == "item.started":
                call.setdefault("start", row["elapsed_seconds"])
            if kind == "item.completed":
                call.update(completed=True, end=row["elapsed_seconds"],
                            failed=_codex_tool_failed(item))
    by_name = Counter(c["name"] for c in calls.values())
    graph = {k.removeprefix("mcp.codebase-memory-mcp."): v for k, v in by_name.items()
             if k.startswith("mcp.codebase-memory-mcp.")}
    complete = all(c["completed"] for c in calls.values()) and malformed == 0
    durations = [c["end"] - c["start"] for c in calls.values()
                 if c["completed"] and "start" in c]
    return {
        "contestant": "codex", "session_id": thread_id,
        "coding_seconds": process["process_wall_seconds"],
        "timing_boundary": "cli_process_start_to_exit",
        "agent_succeeded": (process["exit_code"] == 0 and not process["timed_out"]
                            and turn_completions == 1 and complete),
        "tool_calls": sum(by_name.values()), "tools_by_name": dict(by_name),
        "tool_failures": sum(c.get("failed", False) for c in calls.values()),
        "graph_calls": sum(graph.values()), "graph_by_name": graph,
        "tool_evidence_complete": complete,
        "observed_tool_seconds": sum(durations),
        "tool_duration_samples": len(durations),
        "model_calls": None, "model_attempts": None,
        "model_seconds": None, "user_turns": turn_completions,
        "tokens": ({**dict(usage), "total_input_tokens": usage["input_tokens"],
                    "uncached_input_tokens": usage["input_tokens"] - usage["cached_input_tokens"]}
                   if turn_completions and usage_complete else None),
        "limitations": ["Public Codex events do not expose every model request or retry.",
                        "Tool durations use harness receipt times, not executor clocks."],
    }


def _codex_tool_name(item: dict) -> str | None:
    kind = item.get("type")
    if kind == "mcp_tool_call":
        return "mcp." + item.get("server", "unknown") + "." + item.get("tool", "unknown")
    return {"command_execution": "shell", "file_change": "patch",
            "web_search": "web_search", "collab_tool_call": "subagent",
            "todo_list": "plan"}.get(kind)


def _codex_tool_failed(item: dict) -> bool:
    return (item.get("status") == "failed" or item.get("error") is not None
            or item.get("exit_code") not in (None, 0))


def temper_metrics(summaries: list[dict], sessions: list[dict] | None = None) -> dict:
    """All native attempts count; require every observed terminal/timer."""
    by_name = Counter()
    graph = Counter()
    tokens = Counter()
    model = Counter()
    token_coverage = []
    model_duration_coverage = []
    tool_failures = 0
    walls = []
    complete = bool(summaries)
    statuses = []
    for run in summaries:
        status = (run.get("terminal") or {}).get("status")
        statuses.append(status)
        complete = complete and bool(run.get("trace", {}).get("terminal_event_observed"))
        metrics = run.get("metrics", {})
        token_coverage.append((metrics.get("tokens") or {}).get("coverage"))
        model_duration_coverage.append((metrics.get("model") or {}).get("duration_coverage"))
        tools = metrics.get("tools")
        if tools is None:
            complete = False
        else:
            tool_failures += tools["failed"]
            for name, values in tools.get("by_name", {}).items():
                count = values["calls"]
                by_name[name] += count
                if name.startswith("codebase_memory_"):
                    graph[name.removeprefix("codebase_memory_")] += count
        for key, value in (metrics.get("tokens") or {}).items():
            if isinstance(value, int):
                tokens[key] += value
        for key, value in (metrics.get("model") or {}).items():
            if isinstance(value, int):
                model[key] += value
    session_timing_complete = (bool(sessions)
                               and all(isinstance(s.get("duration_ms"), (int, float))
                                       for s in sessions))
    if session_timing_complete:
        walls = [s["duration_ms"] / 1000 for s in sessions]
    ordered_statuses = [s.get("status") for s in sessions] if sessions else []
    all_models = bool(summaries) and all(s.get("metrics", {}).get("model") for s in summaries)
    all_tokens = bool(summaries) and all(_complete_coverage(c) for c in token_coverage)
    return {
        "contestant": "temper", "attempts": len(summaries),
        "coding_seconds": sum(walls) if session_timing_complete else None,
        "timing_boundary": "sum_of_native_in_process_invocation_duration",
        "agent_succeeded": complete and len(summaries) == len(ordered_statuses)
                           and bool(ordered_statuses) and ordered_statuses[-1] == "succeeded",
        "attempt_statuses": ordered_statuses, "trace_statuses": statuses,
        "tool_calls": sum(by_name.values()),
        "tools_by_name": dict(by_name), "tool_failures": tool_failures,
        "graph_calls": sum(graph.values()), "graph_by_name": dict(graph),
        "tool_evidence_complete": complete,
        "model_calls": model.get("calls") if all_models else None,
        "model_attempts": model.get("attempts") if all_models else None,
        "model_seconds": (model["cumulative_duration_ms"] / 1000
                          if "cumulative_duration_ms" in model
                          and all(_complete_coverage(c) for c in model_duration_coverage) else None),
        "token_coverage": token_coverage, "model_duration_coverage": model_duration_coverage,
        "tokens": ({**dict(tokens),
                    "total_input_tokens": tokens["input_tokens"] + tokens["cache_read_tokens"],
                    "uncached_input_tokens": tokens["input_tokens"],
                    "cached_input_tokens": tokens["cache_read_tokens"]} if all_tokens else None),
        "limitations": ["Native invocation runs in process; standalone daemon startup is setup time.",
                        "Graph wrapper attempts include locally denied calls; provider invocations are separate."],
    }


def comparison(trials: list[dict], expected_pairs: int = 5) -> dict:
    """Do not calculate a success-only median or silently drop a missing pair."""
    cells = {name: [t for t in trials if t["contestant"] == name]
             for name in ("temper", "codex")}
    result = {"schema_version": 1, "expected_pairs": expected_pairs,
              "trials": trials, "contestants": {}, "performance_target_met": False}
    for name, rows in cells.items():
        valid = (len(rows) == expected_pairs
                 and sorted(t["pair"] for t in rows) == list(range(1, expected_pairs + 1))
                 and all(t.get("correct") and t.get("agent_succeeded")
                         and _positive_duration(t.get("coding_seconds")) for t in rows))
        cell = {"attempted": len(rows), "passed": sum(bool(t.get("correct")) for t in rows),
                "complete": valid,
                "tool_evidence_complete": all(
                    all(isinstance(t.get(k), int) and not isinstance(t.get(k), bool) and t[k] >= 0
                        for k in ["tool_calls", "graph_calls"])
                    and t.get("tool_evidence_complete", True)
                    and t.get("mcp", {}).get("complete", False) for t in rows)}
        if valid:
            times = [t["coding_seconds"] for t in rows]
            cell.update(median_seconds=statistics.median(times), min_seconds=min(times),
                        max_seconds=max(times), mean_seconds=statistics.mean(times))
            for metric in ("tool_calls", "graph_calls"):
                values = [t.get(metric) for t in rows]
                cell["median_" + metric] = (statistics.median(values)
                    if all(isinstance(v, int) and not isinstance(v, bool) and v >= 0
                           for v in values) else None)
        result["contestants"][name] = cell
    if all(c["complete"] for c in result["contestants"].values()):
        ratio = (result["contestants"]["temper"]["median_seconds"]
                 / result["contestants"]["codex"]["median_seconds"])
        result.update(temper_to_codex_ratio=ratio,
                      timing_target_met=expected_pairs >= 5 and ratio <= 1,
                      performance_target_met=expected_pairs >= 5 and ratio <= 1
                      and all(c["tool_evidence_complete"] for c in result["contestants"].values()))
    return result


def _positive_duration(value):
    return (isinstance(value, (int, float)) and not isinstance(value, bool)
            and math.isfinite(value) and value > 0)


def _complete_coverage(value):
    return (isinstance(value, dict) and value.get("observed") is not None
            and value.get("observed") == value.get("expected"))


def mcp_metrics(path: Path) -> dict:
    if not path.exists():
        return {"available": False, "provider_calls": None, "complete": False}
    calls, diagnostics = {}, []
    processes, exits = set(), set()
    for line in path.read_text().splitlines():
        row = json.loads(line)
        session = row["proxy_session_id"]
        kind = row["event"]
        if kind == "process_start":
            processes.add(session)
        elif kind == "process_exit":
            exits.add(session)
        elif kind == "transport":
            diagnostics.append(row["reason"])
        if row.get("method") != "tools/call":
            continue
        key = (session, json.dumps(row.get("id"), sort_keys=True))
        if kind == "request" and row.get("direction") == "client_to_provider":
            if key in calls:
                diagnostics.append("duplicate_request_id")
            calls[key] = {"tool": row["tool_name"], "arguments_sha256": row["arguments_sha256"]}
        elif kind == "response" and row.get("request_direction") == "client_to_provider":
            if key not in calls:
                diagnostics.append("unmatched_response")
                continue
            calls[key].update(outcome=row["outcome"], duration_seconds=row["duration_seconds"])
    names = Counter(c["tool"] for c in calls.values())
    signatures = Counter((c["tool"], c["arguments_sha256"]) for c in calls.values())
    outcomes = Counter(c.get("outcome", "missing") for c in calls.values())
    durations = [c["duration_seconds"] for c in calls.values() if c.get("duration_seconds") is not None]
    return {"available": True, "provider_calls": len(calls), "by_name": dict(names),
            "outcomes": dict(outcomes), "index_calls": names["index_repository"],
            "duration_seconds": sum(durations), "duration_samples": len(durations),
            "repeated_identical_requests": sum(n - 1 for n in signatures.values()),
            "complete": bool(processes) and processes == exits and not diagnostics
                        and outcomes["missing"] == 0,
            "diagnostics": diagnostics,
            "limitations": ["Identical arguments can be useful after a source change or during readiness polling.",
                            "Provider calls include harness/worker setup, not only model-selected tools."]}
