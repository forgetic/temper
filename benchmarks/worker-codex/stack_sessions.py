"""Extract every native coding attempt from host-owned monotonic durations."""

import json
from pathlib import Path


def read_agent_sessions(path, *, require_complete=True):
    sessions, pending = [], {}
    for line_number, line in enumerate(Path(path).read_text().splitlines(), 1):
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            if "agent.started" in line or "agent.finished" in line:
                raise ValueError(f"malformed agent boundary event at line {line_number}") from None
            continue
        fields = event.get("fields", {})
        kind = fields.get("event")
        if kind not in {"agent.started", "agent.finished"}:
            continue
        key = (fields.get("artifact.ref"), fields.get("role"), fields.get("kind"))
        if not all(key) or not event.get("timestamp"):
            raise ValueError(f"agent boundary identity/timestamp missing at line {line_number}")
        if kind == "agent.started":
            if key in pending:
                raise ValueError("overlapping agent starts cannot be correlated safely")
            record = {"ordinal": len(sessions) + 1, "artifact_ref": key[0], "role": key[1],
                      "kind": key[2], "runtime_kind": "in_process", "started_at": event["timestamp"],
                      "finished_at": None, "duration_ms": None, "status": None, "complete": False,
                      "correlation": "single-stack sequential artifact/role/kind boundaries"}
            pending[key] = record
            sessions.append(record)
        else:
            if key not in pending:
                raise ValueError("agent finish lacks a matching start")
            duration = fields.get("duration_ms")
            status = fields.get("status")
            if isinstance(duration, bool) or not isinstance(duration, int) or duration < 0:
                raise ValueError("agent finish lacks a valid host monotonic duration_ms")
            if status not in {"succeeded", "failed", "cancelled"}:
                raise ValueError("agent finish lacks a recognized terminal status")
            pending.pop(key).update(finished_at=event["timestamp"], duration_ms=duration,
                                    status=status, terminal_reason=fields.get("reason"), complete=True)
    if require_complete and (not sessions or pending):
        raise ValueError("native coding session boundary evidence is missing or incomplete")
    return sessions
