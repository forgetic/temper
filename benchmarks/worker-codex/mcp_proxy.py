#!/usr/bin/env python3
"""Forward stdio MCP unchanged while recording bounded, payload-free metadata."""

import argparse
from collections import OrderedDict
import hashlib
import json
import os
from pathlib import Path
import re
import select
import signal
import subprocess
import sys
import threading
import time
import uuid

MAX_LINE_BYTES = 2 * 1024 * 1024
MAX_PENDING = 4096
CHUNK_BYTES = 65536
SAFE_NAME = re.compile(r"[A-Za-z0-9_./:-]{1,256}\Z")


def reject_nonfinite(_value):
    raise ValueError("nonfinite JSON number")


def digest(value):
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":"),
                         ensure_ascii=False, allow_nan=False).encode()
    return hashlib.sha256(encoded).hexdigest()


def safe_name(value):
    if isinstance(value, str) and SAFE_NAME.fullmatch(value):
        return value
    return "sha256:" + digest(value)


def safe_id(value):
    if isinstance(value, (str, int, float, type(None))) and len(str(value)) <= 256:
        return value
    return {"sha256": digest(value)}


class Recorder:
    def __init__(self, path):
        self.fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
        self.session_id = str(uuid.uuid4())
        self.lock = threading.RLock()
        self.pending = OrderedDict()

    def record(self, event, **fields):
        with self.lock:
            record = {"schema_version": 1, "proxy_session_id": self.session_id,
                      "monotonic_ns": time.monotonic_ns(), "event": event, **fields}
            data = (json.dumps(record, separators=(",", ":"), allow_nan=False) + "\n").encode()
            written = os.write(self.fd, data)
            if written != len(data):
                raise OSError("incomplete metadata write")

    def message(self, direction, line):
        try:
            message = json.loads(line, parse_constant=reject_nonfinite)
            if not isinstance(message, dict):
                raise ValueError("non-object JSON-RPC message")
            with self.lock:
                self._message(direction, message)
        except (ValueError, UnicodeError, TypeError, AttributeError, RecursionError):
            self.record("transport", direction=direction, reason="invalid_json_rpc")

    def _message(self, direction, message):
        identifier = message.get("id")
        if "method" in message:
            fields = {"direction": direction, "method": safe_name(message["method"])}
            if message["method"] == "tools/call":
                params = message.get("params") or {}
                fields.update(tool_name=safe_name(params.get("name")),
                              arguments_sha256=digest(params.get("arguments", {})))
            if "id" not in message:
                self.record("notification", **fields)
                return
            fields["id"] = safe_id(identifier)
            if len(self.pending) >= MAX_PENDING:
                self.pending.popitem(last=False)
                self.record("transport", direction=direction, reason="pending_limit")
            self.pending[(direction, digest(identifier))] = (time.monotonic_ns(), fields)
            self.record("request", **fields)
            return
        if "id" not in message or not ("result" in message or "error" in message):
            self.record("transport", direction=direction, reason="invalid_json_rpc")
            return
        request_direction = "client_to_provider" if direction == "provider_to_client" else "provider_to_client"
        pending = self.pending.pop((request_direction, digest(identifier)), None)
        result = message.get("result")
        outcome = "rpc_error" if "error" in message else (
            "tool_error" if isinstance(result, dict) and result.get("isError") is True else "success")
        fields = dict(pending[1]) if pending else {"id": safe_id(identifier)}
        fields.update(direction=direction, request_direction=request_direction,
                      correlated=pending is not None, outcome=outcome,
                      success=outcome == "success", is_error=outcome != "success",
                      duration_seconds=(time.monotonic_ns() - pending[0]) / 1e9 if pending else None)
        self.record("response", **fields)

    def close(self):
        os.close(self.fd)


class MessageStream:
    def __init__(self, recorder, direction):
        self.recorder, self.direction = recorder, direction
        self.buffer = bytearray()
        self.dropping = False

    def feed(self, data):
        pieces = data.split(b"\n")
        for index, piece in enumerate(pieces):
            if not self.dropping:
                if len(self.buffer) + len(piece) > MAX_LINE_BYTES:
                    self.buffer.clear()
                    self.dropping = True
                    self.recorder.record("transport", direction=self.direction,
                                         reason="message_too_large", limit_bytes=MAX_LINE_BYTES)
                else:
                    self.buffer.extend(piece)
            if index != len(pieces) - 1:
                if self.buffer and not self.dropping:
                    self.recorder.message(self.direction, self.buffer)
                self.buffer.clear()
                self.dropping = False

    def finish(self):
        if self.buffer and not self.dropping:
            self.recorder.message(self.direction, self.buffer)


def forward(source, destination, child_exited, cancelled, recorder, direction, parse=False,
            close_destination=None):
    stream = MessageStream(recorder, direction) if parse else None
    try:
        while not cancelled.is_set() or not close_destination:
            if not select.select([source], [], [], 0.1)[0]:
                if child_exited.is_set():
                    break
                continue
            data = os.read(source, CHUNK_BYTES)
            if not data:
                break
            if stream:
                stream.feed(data)
            remaining = memoryview(data)
            while remaining:
                if select.select([], [destination], [], 0.1)[1]:
                    count = os.write(destination, remaining[:4096])
                    remaining = remaining[count:]
                elif child_exited.is_set() and close_destination:
                    return
                elif cancelled.is_set():
                    return
    except OSError as error:
        recorder.record("transport", direction=direction, reason="io_error", errno=error.errno)
    finally:
        if stream:
            stream.finish()
        if close_destination:
            try:
                close_destination()
            except OSError:
                pass


def run(command, log):
    log.parent.mkdir(parents=True, exist_ok=True)
    recorder = Recorder(log)
    try:
        child = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                 stderr=subprocess.PIPE, bufsize=0)
    except OSError as error:
        recorder.record("process_exit", exit_code=127, spawn_errno=error.errno)
        recorder.close()
        return 127
    child_exited, cancelled = threading.Event(), threading.Event()
    cancel_timer = None

    def cancel(signum, _frame):
        nonlocal cancel_timer
        cancelled.set()
        try:
            child.send_signal(signum)
        except ProcessLookupError:
            pass
        if cancel_timer is None:
            cancel_timer = threading.Timer(2, child.kill)
            cancel_timer.daemon = True
            cancel_timer.start()

    previous = {sig: signal.signal(sig, cancel) for sig in (signal.SIGINT, signal.SIGTERM)}
    recorder.record("process_start", child_pid=child.pid)
    specs = [(sys.stdin.fileno(), child.stdin.fileno(), "client_to_provider", True, child.stdin.close),
             (child.stdout.fileno(), sys.stdout.fileno(), "provider_to_client", True, None),
             (child.stderr.fileno(), sys.stderr.fileno(), "provider_stderr", False, None)]
    threads = [threading.Thread(target=forward, daemon=True,
               args=(source, destination, child_exited, cancelled, recorder, direction, parse, close))
               for source, destination, direction, parse, close in specs]
    for thread in threads:
        thread.start()
    code = child.wait()
    if cancel_timer:
        cancel_timer.cancel()
    child_exited.set()
    for thread in threads:
        thread.join()
    child.stdout.close()
    child.stderr.close()
    recorder.record("process_exit", exit_code=code)
    recorder.close()
    for sig, handler in previous.items():
        signal.signal(sig, handler)
    return code


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--log", type=Path, required=True)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not args.log.is_absolute():
        parser.error("--log must be an absolute path")
    if not command:
        parser.error("a provider command is required after --")
    code = run(command, args.log)
    if code < 0:
        if -code != signal.SIGKILL:
            signal.signal(-code, signal.SIG_DFL)
        os.kill(os.getpid(), -code)
    return code


if __name__ == "__main__":
    raise SystemExit(main())
