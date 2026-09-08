"""Verify one UUID graph namespace before the timed Codex process starts."""

import hashlib
import json
import os
from pathlib import Path
import selectors
import subprocess
import time
import tomllib
import uuid

from graph_evidence import result_payload


def developer_instructions(config):
    """Preserve the user's existing string when adding infrastructure context."""
    if config.get("profile") is not None:
        raise ValueError("benchmark does not resolve developer_instructions from active Codex profiles")
    value = config.get("developer_instructions", "")
    if not isinstance(value, str):
        raise ValueError("Codex developer_instructions must be a string")
    return value


def current_developer_instructions():
    home = Path(os.environ.get("CODEX_HOME", Path.home() / ".codex"))
    return developer_instructions(tomllib.loads((home / "config.toml").read_text()))


def prepare_namespace(binary, checkout, *, timeout_seconds=30):
    namespace = "worker-codex-" + uuid.uuid4().hex
    instructions = (
        "Benchmark infrastructure: your fresh codebase-memory-mcp project namespace is "
        f"{namespace}. Before project-scoped graph reads, call index_repository with "
        f"repo_path={json.dumps(str(Path(checkout).resolve()))} and name={json.dumps(namespace)}. "
        f"Use project={json.dumps(namespace)} for subsequent graph requests. "
        "This namespace belongs only to this attempt; do not read other graph projects."
    )
    started = time.monotonic()
    proof = {"source": "host_index_status_before_codex", "namespace": namespace,
             "complete": False, "missing_before_start": False, "instructions": instructions,
             "status_request": {"name": "index_status", "arguments": {"project": namespace}},
             "instructions_sha256": hashlib.sha256(instructions.encode()).hexdigest()}
    try:
        result = missing_status([str(binary)], namespace, timeout_seconds=timeout_seconds)
        body = result_payload(result, allow_error=True)
        proof["status_error"] = body.get("error")
        proof["status_is_error"] = result.get("isError")
        if result.get("isError") is not True or body.get("error") != "project not found or not indexed":
            raise ValueError("fresh namespace is not explicitly missing from the provider")
        proof.update(complete=True, missing_before_start=True)
    except (OSError, ValueError, KeyError, TypeError, TimeoutError) as error:
        proof["error"] = f"{type(error).__name__}: {error}"
    proof["setup_seconds"] = time.monotonic() - started
    return proof


def missing_status(command, namespace, *, timeout_seconds=30):
    """A bounded stdio client; it never indexes or signals the shared daemon."""
    process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                               stderr=subprocess.DEVNULL)
    deadline = time.monotonic() + timeout_seconds
    pending = b""
    try:
        with selectors.DefaultSelector() as selector:
            selector.register(process.stdout, selectors.EVENT_READ)

            def send(message):
                process.stdin.write(json.dumps({"jsonrpc": "2.0", **message}).encode() + b"\n")
                process.stdin.flush()

            def receive(identifier):
                nonlocal pending
                while time.monotonic() < deadline:
                    if b"\n" not in pending:
                        if not selector.select(min(0.1, max(0, deadline - time.monotonic()))):
                            continue
                        chunk = os.read(process.stdout.fileno(), 65536)
                        if not chunk:
                            raise ValueError("MCP provider closed before status evidence")
                        pending += chunk
                        if len(pending) > 2 * 1024 * 1024:
                            raise ValueError("MCP status response exceeds capture limit")
                        continue
                    line, pending = pending.split(b"\n", 1)
                    reply = json.loads(line)
                    if not isinstance(reply, dict):
                        raise ValueError("MCP status response must be an object")
                    if "id" not in reply and isinstance(reply.get("method"), str):
                        continue
                    if type(reply.get("id")) is not int or reply["id"] != identifier or "error" in reply:
                        raise ValueError("MCP status response identity or protocol error")
                    return reply["result"]
                raise TimeoutError("MCP namespace check timed out")

            send({"id": 1, "method": "initialize", "params": {
                "protocolVersion": "2024-11-05", "capabilities": {},
                "clientInfo": {"name": "temper-benchmark-namespace", "version": "1"}}})
            if not isinstance(receive(1), dict):
                raise ValueError("MCP initialize result must be an object")
            send({"method": "notifications/initialized"})
            send({"id": 2, "method": "tools/call", "params": {
                "name": "index_status", "arguments": {"project": namespace}}})
            return receive(2)
    finally:
        try:
            process.stdin.close()
        except BrokenPipeError:
            pass
        process.stdout.close()
        try:
            process.wait(timeout=0.5)
        except subprocess.TimeoutExpired:
            process.terminate()
            try:
                process.wait(timeout=1)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=1)
