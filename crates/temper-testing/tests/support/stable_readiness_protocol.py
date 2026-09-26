"""Exercise legacy provider readiness through its actual stdio protocol."""

import json
from pathlib import Path
import subprocess
import sys
import tempfile


def exchange(provider, root, log, profile, calls):
    requests = [
        {"jsonrpc": "2.0", "id": index, "method": "tools/call",
         "params": {"name": name, "arguments": arguments}}
        for index, (name, arguments) in enumerate(calls, start=1)
    ]
    result = subprocess.run(
        [sys.executable, str(provider), str(log), str(root), "fixture",
         "[]", "[]", "0", "-", "0", profile],
        input="".join(json.dumps(request) + "\n" for request in requests),
        text=True, capture_output=True, timeout=10, check=True,
    )
    responses = [json.loads(line) for line in result.stdout.splitlines()]
    assert [response["id"] for response in responses] == list(range(1, len(calls) + 1))
    return [response["result"] for response in responses]


def payload(result, *, error=False):
    assert bool(result.get("isError")) == error, result
    return json.loads(result["content"][0]["text"])


def check_protocol(provider):
    with tempfile.TemporaryDirectory(prefix="temper-stable-readiness-") as directory:
        root = Path(directory)
        first, second = root / "first-checkout", root / "second-checkout"
        first.mkdir()
        second.mkdir()
        log = root / "stable.jsonl"
        project = "temper-stable-fixture"
        results = exchange(provider, first, log, "stable-lifecycle", [
            ("index_status", {"project": project}),
            ("index_repository", {"name": project, "repo_path": str(first)}),
            ("index_status", {"project": project}),
            ("index_status", {"project": "foreign-project"}),
            ("index_repository", {"name": project, "repo_path": str(second)}),
            ("index_status", {"project": project}),
        ])
        assert payload(results[0], error=True) == {"project": project, "status": "missing"}
        assert payload(results[1]) == {"project": project, "status": "indexed"}
        assert payload(results[2]) == {
            "project": project, "status": "ready", "root_path": str(first),
        }
        assert payload(results[3], error=True)["status"] == "missing"
        assert payload(results[4])["project"] == project
        expected = {"project": project, "status": "ready", "root_path": str(second)}
        assert payload(results[5]) == expected
        restarted = exchange(provider, second, log, "stable-lifecycle", [
            ("index_status", {"project": project}),
            ("index_status", {"project": "foreign-project"}),
        ])
        assert payload(restarted[0]) == expected
        assert payload(restarted[1], error=True)["status"] == "missing"
        state = json.loads(Path(str(log) + ".state.json").read_text())
        assert list(state["projects"]) == [project], state
        assert state["counters"] == {"project_creations": 1, "rebinds": 2}, state

        normalized = "normalized-" + project
        control = exchange(provider, first, root / "rebind.jsonl", "stable-rebind", [
            ("index_status", {"project": project}),
            ("index_repository", {"name": project, "repo_path": str(first)}),
            ("index_status", {"project": normalized}),
        ])
        assert payload(control[0]) == {"project": project, "status": "fresh"}
        assert payload(control[1]) == {"project": normalized, "status": "indexed"}
        assert payload(control[2]) == {
            "project": normalized, "status": "ready", "root_path": str(first),
        }
        unknown = exchange(provider, first, root / "unknown.jsonl", "unknown-profile", [
            ("index_repository", {"name": project, "repo_path": str(first)}),
            ("index_status", {"project": project}),
        ])
        assert payload(unknown[1], error=True)["status"] == "missing"
        print(json.dumps({"missing_then_ready": True, "current_root_rebound": True,
                          "restart_retains_binding": True, "foreign_project_missing": True,
                          "normalized_profile_preserved": True, "unknown_profile_not_ready": True}))


if __name__ == "__main__":
    check_protocol(Path(sys.argv[1]).resolve())
