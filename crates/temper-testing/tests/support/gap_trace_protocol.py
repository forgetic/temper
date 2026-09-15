"""Exercise the real gap fixture's qualified trace protocol and stage guards."""

import json
from pathlib import Path
import subprocess
import sys
import tempfile


def check_protocol(provider, seed):
    with tempfile.TemporaryDirectory(prefix="temper-gap-trace-") as root:
        log = Path(root) / "provider.jsonl"
        process = subprocess.Popen(
            [sys.executable, str(provider), str(log), str(seed), "fixture",
             "[]", "[]", "0", "-", "0", "mapped-live-decision-gap-recovery"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True,
        )
        request_id = 0

        def request(method, params):
            nonlocal request_id
            request_id += 1
            process.stdin.write(json.dumps({
                "jsonrpc": "2.0", "id": request_id,
                "method": method, "params": params,
            }) + "\n")
            process.stdin.flush()
            response = json.loads(process.stdout.readline())
            assert response["id"] == request_id, response
            return response["result"]

        def call(tool, **arguments):
            return request("tools/call", {"name": tool, "arguments": arguments})

        def success(tool, **arguments):
            result = call(tool, **arguments)
            assert not result.get("isError"), (tool, result)
            return result.get("structuredContent") or json.loads(result["content"][0]["text"])

        try:
            request("initialize", {})
            requested = "temper-v1-gap-protocol"
            success("index_status", project=requested)
            success("index_repository", name=requested, repo_path=str(seed))
            project = "normalized-" + requested
            success("index_status", project=project)
            roots = [
                success("search_graph", project=project, query=query)["results"][0]["results"]
                for query in ["routing implementation affinity", "focused alias retry behavior"]
            ]
            first, second = [root[0]["qualifiedName"] for root in roots]
            success("get_code_snippet", project=project, qualified_name=first)
            negative_count = 0
            for selected, sibling, expected_callers in [(first, second, []), (second, first, [roots[1][1]["qualifiedName"]])]:
                if selected == second:
                    success("get_code_snippet", project=project, qualified_name=second)
                terminal = selected.rsplit("::", 1)[1]
                for invalid in [terminal, sibling, "other::" + terminal, selected + "_other"]:
                    result = call("trace_path", project=project, function_name=invalid)
                    assert result.get("isError"), "unselected trace identity was accepted"
                    negative_count += 1
                trace = success("trace_path", project=project, function_name=selected)
                assert trace["function"]["qualifiedName"] == selected, trace
                assert trace["function"]["name"] == terminal, trace
                assert [caller["qualified_name"] for caller in trace["callers"]] == expected_callers, trace
            success("get_code_snippet", project=project, qualified_name=roots[1][1]["qualifiedName"])
            success("get_code_snippet", project=project, qualified_name=roots[0][2]["qualifiedName"])
            print(json.dumps({"qualified_traces": 2, "rejected_unselected_traces": negative_count,
                              "caller_and_focused_sources_completed": True}))
        finally:
            process.stdin.close()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
            process.stdout.close()
            process.stderr.close()


if __name__ == "__main__":
    check_protocol(Path(sys.argv[1]), Path(sys.argv[2]).resolve())
