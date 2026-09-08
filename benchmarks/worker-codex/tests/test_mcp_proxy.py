from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest


PROXY = Path(__file__).resolve().parents[1] / "mcp_proxy.py"
SERVER = r'''
import json, sys
sys.stderr.buffer.write(b"provider stderr\xff\n")
sys.stderr.buffer.flush()
for line in sys.stdin.buffer:
    request = json.loads(line)
    if "id" not in request:
        continue
    method = request.get("method")
    if method == "initialize":
        result = {"protocolVersion": "2025-03-26", "capabilities": {"tools": {}}}
    elif method == "tools/list":
        result = {"tools": [{"name": "get_code_snippet", "inputSchema": {"type": "object"}}]}
    elif request.get("params", {}).get("name") == "rpc_failure":
        print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32000, "message": "PRIVATE ERROR"}}), flush=True)
        continue
    else:
        result = {"content": [{"type": "text", "text": "PRIVATE SOURCE"}],
                  "isError": request.get("params", {}).get("name") == "tool_failure"}
    print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
'''


def wire(messages):
    return b"".join(json.dumps(message).encode() + b"\n" for message in messages)


def request(identifier, method, params=None):
    result = {"jsonrpc": "2.0", "id": identifier, "method": method}
    if params is not None:
        result["params"] = params
    return result


class ProxyTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="mcp-proxy-test-")
        self.addCleanup(self.temporary.cleanup)
        self.log = Path(self.temporary.name) / "events.jsonl"

    def command(self, server=SERVER):
        return [sys.executable, str(PROXY), "--log", str(self.log), "--",
                sys.executable, "-u", "-c", server]

    def run_server(self, messages, server=SERVER):
        return subprocess.run(self.command(server), input=wire(messages),
                              capture_output=True, timeout=10, check=False)

    def records(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()]

    def test_handshake_calls_pair_ids_and_keep_payloads_private(self):
        arguments = {"qualified_name": "PRIVATE QUALIFIED NAME", "project": "fixture"}
        messages = [request(1, "initialize"), request("1", "tools/list"),
                    {"jsonrpc": "2.0", "method": "notifications/initialized"},
                    request(2, "tools/call", {"name": "get_code_snippet", "arguments": arguments}),
                    request(3, "tools/call", {"name": "tool_failure", "arguments": {}}),
                    request(4, "tools/call", {"name": "rpc_failure", "arguments": {}})]
        result = self.run_server(messages)
        direct = subprocess.run([sys.executable, "-u", "-c", SERVER], input=wire(messages),
                                capture_output=True, timeout=10, check=False)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stdout, direct.stdout)
        self.assertEqual(result.stderr, direct.stderr)
        records = self.records()
        requests = [event for event in records if event["event"] == "request"]
        responses = [event for event in records if event["event"] == "response"]
        self.assertEqual([event["method"] for event in requests],
                         ["initialize", "tools/list", "tools/call", "tools/call", "tools/call"])
        self.assertEqual([event["id"] for event in responses], [1, "1", 2, 3, 4])
        self.assertEqual([event["outcome"] for event in responses],
                         ["success", "success", "success", "tool_error", "rpc_error"])
        self.assertTrue(all(event["correlated"] for event in responses))
        self.assertTrue(all(event["duration_seconds"] >= 0 for event in responses))
        canonical = json.dumps(arguments, sort_keys=True, separators=(",", ":")).encode()
        self.assertEqual(requests[2]["arguments_sha256"], hashlib.sha256(canonical).hexdigest())
        self.assertEqual(requests[2]["tool_name"], "get_code_snippet")
        self.assertNotIn("PRIVATE", self.log.read_text())
        self.assertEqual(len({event["proxy_session_id"] for event in records}), 1)
        self.assertTrue(all(isinstance(event["monotonic_ns"], int) for event in records))
        self.assertEqual(records[-1]["exit_code"], 0)

    def test_malformed_and_oversized_messages_are_forwarded_unchanged(self):
        server = r'''
import sys
sys.stdin.buffer.read()
sys.stdout.buffer.write(b"not json\n" + b"x" * (2 * 1024 * 1024 + 1) + b"\n")
sys.stdout.buffer.write(b'{"jsonrpc":"2.0","id":9,"result":{}}\n')
'''
        result = self.run_server([request(9, "tools/list")], server)
        expected = b"not json\n" + b"x" * (2 * 1024 * 1024 + 1) + b"\n"
        expected += b'{"jsonrpc":"2.0","id":9,"result":{}}\n'
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stdout, expected)
        records = self.records()
        self.assertEqual([event["reason"] for event in records if event["event"] == "transport"],
                         ["invalid_json_rpc", "message_too_large"])
        self.assertTrue(next(event for event in records if event["event"] == "response")["correlated"])
        self.assertLess(self.log.stat().st_size, 10000)

    def test_eof_closes_provider_stdin_and_preserves_exit_status(self):
        server = 'import sys; data=sys.stdin.buffer.read(); sys.stdout.buffer.write(data); sys.exit(7)'
        messages = [request(1, "tools/list")]
        result = self.run_server(messages, server)
        self.assertEqual(result.returncode, 7)
        self.assertEqual(result.stdout, wire(messages))
        self.assertEqual(self.records()[-1]["exit_code"], 7)

    def test_provider_exit_does_not_wait_for_client_stdin_eof(self):
        process = subprocess.Popen(self.command('import sys; sys.exit(5)'), stdin=subprocess.PIPE,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            self.assertEqual(process.wait(timeout=5), 5)
        finally:
            process.stdin.close()
            if process.poll() is None:
                process.kill()
                process.wait()

    def test_append_log_sessions_have_distinct_identity(self):
        with ThreadPoolExecutor(max_workers=2) as executor:
            results = list(executor.map(lambda _: self.run_server([request(1, "initialize")]), range(2)))
        self.assertTrue(all(result.returncode == 0 for result in results))
        records = self.records()
        self.assertEqual(len({event["proxy_session_id"] for event in records}), 2)
        self.assertEqual(sum(event["event"] == "process_exit" for event in records), 2)

    def test_out_of_order_response_ids_are_correlated(self):
        server = r'''
import json,sys
requests = [json.loads(line) for line in sys.stdin.buffer]
for request in reversed(requests):
    print(json.dumps({"jsonrpc":"2.0", "id":request["id"], "result":{}}))
'''
        result = self.run_server([request(1, "initialize"), request("1", "tools/list")], server)
        self.assertEqual(result.returncode, 0)
        responses = [event for event in self.records() if event["event"] == "response"]
        self.assertEqual([(event["id"], event["method"]) for event in responses],
                         [("1", "tools/list"), (1, "initialize")])
        self.assertTrue(all(event["correlated"] for event in responses))

    def test_cancellation_stops_provider_that_ignores_termination(self):
        server = ('import signal,time; signal.signal(signal.SIGTERM, signal.SIG_IGN); '
                  'print("ready", flush=True); time.sleep(30)')
        process = subprocess.Popen(self.command(server), stdin=subprocess.PIPE,
                                   stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        try:
            self.assertEqual(process.stdout.readline(), b"ready\n")
            process.terminate()
            self.assertEqual(process.wait(timeout=5), -signal.SIGKILL)
            self.assertEqual(self.records()[-1]["exit_code"], -signal.SIGKILL)
        finally:
            process.stdin.close()
            process.stdout.close()
            if process.poll() is None:
                process.kill()
            process.wait()

    def test_cancellation_preserves_signal_status_and_leaves_unrelated_child(self):
        unrelated = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"])
        process = subprocess.Popen(self.command('import time; time.sleep(30)'), stdin=subprocess.PIPE,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                if self.log.exists() and any(event["event"] == "process_start" for event in self.records()):
                    break
                time.sleep(0.01)
            else:
                self.fail("proxy did not start")
            process.terminate()
            self.assertEqual(process.wait(timeout=5), -signal.SIGTERM)
            self.assertIsNone(unrelated.poll())
            self.assertEqual(self.records()[-1]["exit_code"], -signal.SIGTERM)
        finally:
            process.stdin.close()
            for child in (process, unrelated):
                if child.poll() is None:
                    child.kill()
                child.wait()


if __name__ == "__main__":
    unittest.main()
