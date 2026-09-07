"""Private live fixture: public MCP frontends, cold shared service, real descendants.

This models shared service work, not native same-request index coalescing.
The harness only writes this file. The first frontend starts the daemon.
"""
import fcntl
import hashlib
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import threading
import time

LOG = sys.argv[1]
ADDRESS = "\0temper-live-" + hashlib.sha256(LOG.encode()).hexdigest()[:32]
LOCK = threading.Lock()
WORK_LOCK = threading.Lock()
ROOTS = {}
SESSIONS = {}
HAD_SESSION = False
EMPTY_SINCE = None
TOKENS = {"implementation": "fixture::retry_worker_topic", "caller": "fixture::dispatch", "focused_test": "fixture::alias_retries_keep_the_original_ordered_worker"}
FILES = {"implementation": "src/lib.rs", "caller": "src/caller.rs", "focused_test": "tests/retry_affinity.rs"}


def identity(pid):
    fields = Path(f"/proc/{pid}/stat").read_text().rsplit(") ", 1)[1].split()
    return {"pid": pid, "start": int(fields[19])}


def record(event, **fields):
    row = json.dumps({"event": event, "at": time.monotonic_ns(), **fields}) + "\n"
    with open(LOG, "a") as output:
        fcntl.flock(output, fcntl.LOCK_EX)
        output.write(row)
        output.flush()


def send(stream, value):
    stream.write((json.dumps(value) + "\n").encode())
    stream.flush()


def work_process(channel):
    # The physical shared work actor acknowledges every B query, making a
    # surviving daemon or echoed request insufficient to pass the live proof.
    stream = channel.makefile("rwb")
    for line in stream:
        request = json.loads(line)
        send(stream, {"nonce": request["nonce"], "worker": identity(os.getpid())})
    stream.close()
    channel.close()
    os._exit(0)


def make_worker():
    parent, child = socket.socketpair()
    pid = os.fork()
    if pid == 0:
        parent.close()
        work_process(child)
    child.close()
    record("work_started", identity=identity(pid))
    return pid, parent, parent.makefile("rwb")


def result(payload, error=False):
    return {"content": [{"type": "text", "text": json.dumps(payload)}], "structuredContent": payload, "isError": error}


def tools():
    fields = {"project": {"type": "string"}, "query": {"type": "string"}, "pattern": {"type": "string"}, "name_pattern": {"type": "string"}, "name": {"type": "string"}, "repo_path": {"type": "string"}, "qualified_name": {"type": "string"}, "function_name": {"type": "string"}, "direction": {"type": "string"}, "mode": {"type": "string"}, "limit": {"type": "integer"}, "depth": {"type": "integer"}}
    required = {"index_status": ["project"], "index_repository": ["repo_path"], "get_code_snippet": ["project", "qualified_name"], "trace_path": ["project", "function_name"]}
    return [{"name": name, "description": "Bounded live lifecycle fixture " + name, "inputSchema": {"type": "object", "properties": fields, "required": required.get(name, [])}} for name in ("list_projects", "index_status", "index_repository", "search_graph", "trace_path", "get_code_snippet", "search_code", "delete_project")]


def request_result(request, session, worker):
    method = request.get("method")
    if method == "initialize":
        return {"protocolVersion": "2024-11-05", "serverInfo": {"name": "codebase-memory-mcp", "version": "0.10.8"}, "capabilities": {"tools": {}}}
    if method == "tools/list":
        return {"tools": tools()}
    params = request.get("params") or {}
    name, args = params.get("name"), params.get("arguments") or {}
    project = args.get("project", "")
    with LOCK:
        root = ROOTS.get(project)
        if name == "list_projects":
            payload = {"projects": [{"name": key, "root_path": value} for key, value in ROOTS.items()], "has_more": False}
        elif name == "index_status":
            payload = {"project": project, "status": "ready" if root else "missing"}
            if root:
                payload["root_path"] = root
        elif name == "index_repository":
            root = str(Path(args["repo_path"]).resolve())
            project = args["name"]
            ROOTS[project] = root
            payload = {"project": project, "status": "indexed"}
            record("indexed", session=session, root=root, project=project)
        else:
            payload = None
    if payload is not None:
        return result(payload, name == "index_status" and root is None)
    if not root:
        return result({"error": "unbound project"}, True)
    with WORK_LOCK:
        nonce = time.monotonic_ns()
        send(worker, {"nonce": nonce})
        acknowledgment = json.loads(worker.readline())
        if acknowledgment.get("nonce") != nonce:
            raise RuntimeError("shared work acknowledgment mismatch")
    if name == "search_graph":
        payload = {"results": [{"qualified_name": TOKENS[kind], "file_path": FILES[kind], "is_test": kind == "focused_test"} for kind in ("implementation", "focused_test")], "total": 2, "has_more": False}
    elif name == "trace_path" and args.get("function_name") == TOKENS["implementation"]:
        payload = {"qualified_name": TOKENS["implementation"], "file_path": FILES["implementation"], "callers": [{"qualified_name": TOKENS["caller"], "file_path": FILES["caller"]}], "has_more": False}
    elif name == "get_code_snippet" and args.get("qualified_name") in TOKENS.values():
        kind = next(key for key, token in TOKENS.items() if token == args["qualified_name"])
        source = Path(root, FILES[kind]).read_text()
        payload = {"qualified_name": TOKENS[kind], "file_path": FILES[kind], "source": source, "start_line": 1, "end_line": len(source.splitlines()), "is_test": kind == "focused_test"}
    else:
        return result({"error": "unexpected fixture selector"}, True)
    record("graph_result", session=session, frontend=SESSIONS[session]["identity"], tool=name, root=root, worker=acknowledgment["worker"], selector=payload.get("qualified_name"), source=payload.get("source"))
    return result(payload)


def serve_client(connection, worker):
    global HAD_SESSION, EMPTY_SINCE
    stream = connection.makefile("rwb")
    session = None
    try:
        hello = json.loads(stream.readline())
        session = str(hello["identity"]["pid"])
        with LOCK:
            SESSIONS[session] = hello
            HAD_SESSION = True
            EMPTY_SINCE = None
            record("attached", session=session, active=len(SESSIONS), **hello)
        for line in stream:
            request = json.loads(line)
            if "id" not in request:
                continue
            response = request_result(request, session, worker)
            send(stream, {"jsonrpc": "2.0", "id": request["id"], "result": response})
    except (BrokenPipeError, ConnectionResetError):
        pass
    except Exception as error:
        record("protocol_failure", category=type(error).__name__)
    finally:
        if session is not None:
            with LOCK:
                SESSIONS.pop(session, None)
                record("detached", session=session, active=len(SESSIONS))
                if not SESSIONS:
                    EMPTY_SINCE = time.monotonic()
        stream.close()
        connection.close()


def daemon():
    worker_pid, channel, worker = make_worker()
    record("daemon_started", identity=identity(os.getpid()), parent=identity(os.getppid()))
    listener = socket.socket(socket.AF_UNIX)
    listener.bind(ADDRESS)
    listener.listen(16)
    listener.settimeout(0.05)
    deadline = time.monotonic() + 180
    try:
        while time.monotonic() < deadline:
            with LOCK:
                empty = HAD_SESSION and not SESSIONS and EMPTY_SINCE is not None
            if empty:
                record("last_session_closed")
                break
            try:
                client, _ = listener.accept()
            except socket.timeout:
                continue
            threading.Thread(target=serve_client, args=(client, worker), daemon=True).start()
        else:
            record("fixture_deadline_expired")
    finally:
        listener.close()
        worker.close()
        channel.close()
        os.waitpid(worker_pid, 0)
        record("work_exited")
        record("daemon_exited")


def frontend():
    analysis = "--tool-profile=analysis" in sys.argv
    with open(LOG + ".lock", "w") as admission:
        fcntl.flock(admission, fcntl.LOCK_EX)
        connection = socket.socket(socket.AF_UNIX)
        try:
            connection.connect(ADDRESS)
        except ConnectionRefusedError:
            subprocess.Popen([sys.executable, "-u", __file__, LOG, "--fixture-daemon"], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            deadline = time.monotonic() + 5
            while True:
                try:
                    connection.connect(ADDRESS)
                    break
                except ConnectionRefusedError:
                    if time.monotonic() >= deadline:
                        raise
                    time.sleep(0.01)
        stream = connection.makefile("rwb")
        ancestor = os.getppid()
        owners = []
        for _ in range(12):
            try:
                command = Path(f"/proc/{ancestor}/cmdline").read_bytes()
                if b"--temper-codebase-memory-bootstrap" in command.split(b"\0"):
                    owners.append(identity(ancestor))
                ancestor = int(Path(f"/proc/{ancestor}/stat").read_text().rsplit(") ", 1)[1].split()[1])
            except (OSError, ValueError):
                break
        send(stream, {"identity": identity(os.getpid()), "parent": identity(os.getppid()), "owners": owners, "profile": "analysis" if analysis else "serving", "cwd": str(Path.cwd().resolve())})
    try:
        for line in sys.stdin:
            request = json.loads(line)
            send(stream, request)
            if "id" in request:
                response = stream.readline()
                if not response:
                    raise RuntimeError("daemon disconnected")
                sys.stdout.buffer.write(response)
                sys.stdout.buffer.flush()
    finally:
        stream.close()
        connection.close()


if __name__ == "__main__":
    if sys.argv[2:3] == ["--fixture-daemon"]:
        daemon()
    else:
        frontend()
