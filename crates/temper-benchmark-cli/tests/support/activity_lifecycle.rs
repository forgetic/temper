use std::fs;
use std::path::Path;

use serde_json::Value;

pub(super) fn write_activity_emitter(root: &Path) {
    // Both benchmark modes opt into the known first-party contract. Their fake
    // successful agents must emit the lifecycle of the real coding agent.
    fs::write(
        root.join("emit-activity.py"),
        r#"import json
import socket
import sys

def frame(elapsed_ms, event):
    return {
        "version": 1,
        "occurred_at": "2026-08-01T00:00:00Z",
        "elapsed_ms": elapsed_ms,
        "scope": {"id": "fake-main", "kind": "main"},
        "event": event,
    }

frames = [
    frame(0, {"type": "scope.started", "data": {}}),
    frame(1, {"type": "scope.finished", "data": {"status": "succeeded", "duration_ms": 1}}),
]
host, port = sys.argv[1].rsplit(":", 1)
with socket.create_connection((host, int(port)), timeout=5) as stream:
    for value in frames:
        stream.sendall(json.dumps(value).encode() + b"\n")
    stream.shutdown(socket.SHUT_WR)
    if stream.recv(1):
        raise RuntimeError("unexpected activity endpoint response")
"#,
    )
    .unwrap();
}

pub(super) fn assert_successful_lifecycle(root: &Path) {
    let records = fs::read_to_string(root.join("trace.export.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|row| row["type"] == "agent_run_event_v1")
        .collect::<Vec<_>>();
    let events = records
        .iter()
        .map(|row| row["event"]["event"]["type"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        events,
        [
            "run.started",
            "scope.started",
            "scope.finished",
            "run.finished"
        ]
    );
    assert_eq!(records[1]["event"]["scope"]["kind"], "main");
    assert_eq!(records[1]["event"]["scope"], records[2]["event"]["scope"]);
    for record in &records[2..] {
        assert_eq!(record["event"]["event"]["data"]["status"], "succeeded");
    }
}
