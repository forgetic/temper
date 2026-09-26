"""Exercise legacy provider readiness through its actual stdio protocol."""

import json
from pathlib import Path
import subprocess
import sys
import tempfile


def exchange(provider, root, log, profile, calls):
    requests = [
        {"jsonrpc": "2.0", "id": index,
         "method": "tools/list" if name == "tools/list" else "tools/call",
         "params": {"name": name, "arguments": arguments}}
        for index, (name, arguments) in enumerate(calls, start=1)
    ]
    result = subprocess.run(
        [sys.executable, str(provider), str(log), str(root), "fixture",
         '["search_code","index_status"]', '["index_repository"]', "0", "-", "0", profile],
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
        for checkout in (first, second):
            for relative in ("src/lib.rs", "src/caller.rs", "tests/retry_affinity.rs"):
                path = checkout / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(f"{checkout.name}: {relative} source\n")
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

        chain = [
            ("search_graph", {"query": "alias retry worker affinity"}),
            ("search_code", {"pattern": "retry_worker_topic"}),
            ("get_code_snippet", {"qualified_name": "retry_worker_topic"}),
            ("trace_path", {"function_name": "retry_worker_topic", "direction": "inbound"}),
            ("get_code_snippet", {"qualified_name": "dispatch"}),
            ("get_code_snippet", {"qualified_name": "alias_retries_keep_the_original_ordered_worker"}),
        ]
        bound = [(name, dict(arguments, project=project)) for name, arguments in chain]
        search = exchange(provider, second, log, "stable-lifecycle", [("tools/list", {})] + bound)
        schema = next(tool["inputSchema"] for tool in search[0]["tools"] if tool["name"] == "search_code")
        assert schema["required"] == ["pattern"]
        for result in search[1:]:
            assert payload(result) == result["structuredContent"]
        for index, relative in [(3, "src/lib.rs"), (5, "src/caller.rs"), (6, "tests/retry_affinity.rs")]:
            source = payload(search[index])
            assert source["file_path"] == relative
            assert source["source"] == (second / relative).read_text()
        for name, arguments in [
            ("search_graph", {"project": "foreign-project", "query": "alias retry worker affinity"}),
            ("search_code", {"project": project, "pattern": "retry_worker_topic"}),
        ]:
            invalid = exchange(provider, second, log, "stable-lifecycle", [(name, arguments)])
            assert invalid[0]["isError"] is True
        for arguments in [{"query": "retry_worker_topic"}, {"pattern": "absent-symbol"}]:
            invalid = exchange(provider, second, log, "stable-lifecycle", [bound[0], ("search_code", dict(arguments, project=project))])
            assert invalid[1]["isError"] is True
        (second / "src/caller.rs").unlink()
        missing_source = exchange(provider, second, log, "stable-lifecycle", bound[:5])
        assert missing_source[4]["isError"] is True

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
                          "normalized_profile_preserved": True, "unknown_profile_not_ready": True,
                          "structured_chain_uses_rebound_sources": True,
                          "invalid_pattern_or_source_rejected": True}))


if __name__ == "__main__":
    check_protocol(Path(sys.argv[1]).resolve())
