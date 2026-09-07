"""Run the installed Codex CLI and retain timestamped public event evidence."""

from __future__ import annotations

import json
import os
from pathlib import Path
import selectors
import signal
import subprocess
import sys
import time


def run_codex(
    checkout: Path,
    task: str,
    output: Path,
    *,
    executable: str = "codex",
    timeout_seconds: float = 1800,
    mcp_proxy: Path | None = None,
    mcp_binary: Path | None = None,
) -> dict:
    """Measure a single fresh session, including CLI/tool startup and validation.

    Keep the user's Codex configuration and explicitly pin model/effort. Disable
    the unrelated Forgejo connector: the harness owns delivery, and the coding
    contestant needs only its local checkout and codebase-memory MCP.
    """
    output.mkdir(parents=True, exist_ok=False)
    command = [
        executable, "exec", "--dangerously-bypass-approvals-and-sandbox",
        "--json", "--ephemeral", "--color", "never",
        "--model", "gpt-6-astra",
        "-c", 'model_reasoning_effort="xhigh"',
        "-c", "mcp_servers.forgejo.enabled=false",
        "--output-last-message", str(output / "final-message.txt"),
    ]
    if mcp_proxy is not None:
        if mcp_binary is None:
            raise ValueError("MCP recording requires an explicit provider binary")
        proxy_args = [str(mcp_proxy.resolve()), "--log", str(output / "mcp.jsonl"),
                      "--", str(mcp_binary.resolve())]
        command += ["-c", "mcp_servers.codebase-memory-mcp.command=" + json.dumps(sys.executable),
                    "-c", "mcp_servers.codebase-memory-mcp.args=" + json.dumps(proxy_args)]
    command.append("-")
    prompt = task + (
        "\n\nHarness delivery instructions: implement this task in the current "
        "repository, run the required formatting and tests, inspect your diff, "
        "and finish. Leave changes in the checkout; the harness owns Git commit, "
        "push, and PR delivery. Do not use submit_for_pr (it is a Temper tool). "
        "Use codebase-memory-mcp for code discovery as directed by AGENTS.md.\n"
    )
    (output / "prompt.txt").write_text(prompt)
    (output / "command.json").write_text(json.dumps(command, indent=2) + "\n")
    started = time.monotonic()
    started_unix = time.time()
    timed_out = False
    with (output / "events.jsonl").open("w") as events, (
        output / "stderr.log"
    ).open("wb") as stderr:
        process = subprocess.Popen(
            command, cwd=checkout, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=stderr, start_new_session=True,
        )
        try:
            process.stdin.write(prompt.encode())
            process.stdin.close()
            os.set_blocking(process.stdout.fileno(), False)
            with selectors.DefaultSelector() as selector:
                selector.register(process.stdout, selectors.EVENT_READ)
                pending = b""
                while selector.get_map():
                    if time.monotonic() - started >= timeout_seconds:
                        timed_out = True
                        _terminate(process)
                        break
                    for key, _ in selector.select(timeout=0.2):
                        chunk = os.read(key.fd, 65536)
                        if not chunk:
                            selector.unregister(key.fileobj)
                            continue
                        pending += chunk
                        while b"\n" in pending:
                            line, pending = pending.split(b"\n", 1)
                            _record(events, line, started)
                if pending:
                    _record(events, pending, started)
            remaining = max(0.1, timeout_seconds - (time.monotonic() - started))
            try:
                exit_code = process.wait(timeout=remaining)
            except subprocess.TimeoutExpired:
                timed_out = True
                _terminate(process)
                exit_code = process.returncode
        finally:
            if process.poll() is None:
                _terminate(process)
            process.stdout.close()
    result = {
        "schema_version": 1, "contestant": "codex",
        "model": "gpt-6-astra", "reasoning_effort": "xhigh",
        "started_unix": started_unix, "ended_unix": time.time(),
        "process_wall_seconds": time.monotonic() - started,
        "exit_code": exit_code, "timed_out": timed_out,
    }
    (output / "process.json").write_text(json.dumps(result, indent=2) + "\n")
    return result


def _record(stream, line: bytes, started: float) -> None:
    if not line.strip():
        return
    try:
        event = json.loads(line)
    except (ValueError, UnicodeDecodeError):
        event = {"type": "capture.invalid_json", "raw": line.decode(errors="replace")}
    stream.write(json.dumps({"elapsed_seconds": time.monotonic() - started,
                             "event": event}) + "\n")
    stream.flush()


def _terminate(process: subprocess.Popen) -> None:
    """Terminate only the session group started by this harness."""
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.wait(timeout=5)
