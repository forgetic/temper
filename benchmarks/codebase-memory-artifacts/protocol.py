"""Public newline-delimited MCP, with bounded reads and private raw evidence."""
import json
import os
import select
import subprocess
import time

from contract import Refusal


class McpClient:
    def __init__(self, executable, root, environment, uid, gid, profile, timeout, raw, name):
        self.timeout = timeout
        self.raw = raw
        self.name = name
        self.serial = 0
        self.buffer = bytearray()
        self.calls = []
        self.closed = False
        self.forced_cleanup = False
        self.startup_seconds = 0.0
        self.stderr = open(raw.parent / (name + "-stderr.log"), "wb")
        arguments = [str(executable)] + (["--tool-profile=" + profile] if profile != "all" else [])
        try:
            self.process = subprocess.Popen(arguments, cwd=root,
                                            env=environment, user=uid, group=gid, extra_groups=[],
                                            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.stderr)
        except OSError as error:
            self.stderr.close()
            raise Refusal("provider-spawn-failed") from error
        self.started = time.monotonic()

    def initialize(self):
        try:
            initialized = self.request("initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                                                       "clientInfo": {"name": "temper-artifact-benchmark", "version": "1"}})
            if initialized.get("serverInfo", {}).get("version") != "0.10.8":
                raise Refusal("runtime-provider-version-mismatch")
            self.notify("notifications/initialized", {})
            tools = self.request("tools/list", {})
            if not isinstance(tools.get("tools"), list):
                raise Refusal("tool-discovery-shape")
            self.startup_seconds = time.monotonic() - self.started
        except BaseException:
            self.startup_seconds = time.monotonic() - self.started
            self.close()
            raise

    def record(self, direction, payload):
        with self.raw.open("a") as target:
            target.write(json.dumps({"client": self.name, "direction": direction, "payload": payload}) + "\n")

    def notify(self, method, params):
        self._write({"jsonrpc": "2.0", "method": method, "params": params})

    def _write(self, message):
        self.record("request", message)
        try:
            self.process.stdin.write(json.dumps(message).encode() + b"\n")
            self.process.stdin.flush()
        except (OSError, BrokenPipeError) as error:
            raise Refusal("provider-input-closed") from error

    def _read(self, deadline):
        while b"\n" not in self.buffer:
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not select.select([self.process.stdout], [], [], remaining)[0]:
                raise Refusal("provider-request-timeout")
            block = os.read(self.process.stdout.fileno(), 65536)
            if not block:
                raise Refusal("provider-output-closed")
            self.buffer.extend(block)
            if len(self.buffer) > 16 * 1024 * 1024:
                raise Refusal("provider-response-too-large")
        line, _, tail = self.buffer.partition(b"\n")
        self.buffer = bytearray(tail)
        try:
            response = json.loads(line)
        except ValueError as error:
            raise Refusal("provider-response-malformed") from error
        self.record("response", response)
        return response

    def request(self, method, params):
        self.serial += 1
        self._write({"jsonrpc": "2.0", "id": self.serial, "method": method, "params": params})
        deadline = time.monotonic() + self.timeout
        while True:
            response = self._read(deadline)
            if response.get("id") != self.serial:
                if response.get("method") == "ping" and "id" in response:
                    self._write({"jsonrpc": "2.0", "id": response["id"], "result": {}})
                continue
            if "error" in response or "result" not in response:
                raise Refusal("provider-rpc-error")
            return response["result"]

    def call(self, tool, arguments):
        started = time.monotonic()
        entry = {"tool": tool, "seconds": None, "success": False}
        self.calls.append(entry)
        try:
            result = self.request("tools/call", {"name": tool, "arguments": arguments})
            if result.get("isError"):
                raise Refusal("provider-tool-error")
            content = result.get("content", [])
            if len(content) != 1 or content[0].get("type") != "text":
                raise Refusal("provider-content-shape")
            try:
                parsed = json.loads(content[0]["text"])
            except ValueError as error:
                raise Refusal("provider-content-not-json") from error
            if not isinstance(parsed, dict) or "error" in parsed:
                raise Refusal("provider-content-error")
            entry["success"] = True
            return parsed
        finally:
            entry["seconds"] = time.monotonic() - started

    def close(self):
        if self.closed:
            return
        self.closed = True
        try:
            try:
                self.process.stdin.close()
            except OSError:
                # A provider can exit after a failed buffered write. Closing
                # that buffer must never bypass wait/descendant accounting.
                pass
            try:
                self.process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                self.forced_cleanup = True
                self.process.terminate()
                try:
                    self.process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    self.process.kill()
                    self.process.wait()
        finally:
            for stream in (self.process.stdout, self.stderr):
                try:
                    stream.close()
                except OSError:
                    pass
