"""Private file, subprocess, and TOML helpers for the isolated delivery stack."""

import json
import os
from pathlib import Path
import signal
import subprocess
import time
import tomllib


def private_json(path, value):
    path = Path(path)
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(descriptor, "w") as output:
        json.dump(value, output, indent=2)
        output.write("\n")


def write_toml(path, document):
    """Serialize the scalar/table/array-of-table subset used by Temper config."""
    lines = []

    def scalar(value):
        if isinstance(value, bool):
            return str(value).lower()
        if isinstance(value, (str, int, float)):
            return json.dumps(value)
        if isinstance(value, list):
            return "[" + ", ".join(scalar(item) for item in value) + "]"
        raise TypeError(f"unsupported TOML value type: {type(value).__name__}")

    def table(value, prefix, array=False):
        if prefix:
            name = ".".join(json.dumps(part) for part in prefix)
            lines.append(("[[" if array else "[") + name + ("]]" if array else "]"))
        children = []
        for key, item in value.items():
            if isinstance(item, dict) or (isinstance(item, list) and item and isinstance(item[0], dict)):
                children.append((key, item))
            else:
                lines.append(f"{json.dumps(key)} = {scalar(item)}")
        lines.append("")
        for key, item in children:
            if isinstance(item, list):
                for entry in item:
                    table(entry, prefix + [key], True)
            else:
                table(item, prefix + [key])

    table(document, [])
    text = "\n".join(lines)
    if tomllib.loads(text) != document:
        raise ValueError("TOML round-trip changed configuration")
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(descriptor, "w") as output:
        output.write(text)


def run_logged(argv, log, *, cwd=None, env=None, timeout=180):
    with Path(log).open("ab") as output:
        completed = subprocess.run(argv, cwd=cwd, env=env, stdin=subprocess.DEVNULL,
                                   stdout=output, stderr=subprocess.STDOUT, timeout=timeout)
    if completed.returncode:
        raise RuntimeError(f"{Path(str(argv[0])).name} exited {completed.returncode}; inspect {log}")


def git(argv, *, cwd, token=None, log=None, extra_env=None):
    env = os.environ.copy()
    env["GIT_TERMINAL_PROMPT"] = "0"
    if extra_env:
        env.update(extra_env)
    if token:
        env.update(GIT_CONFIG_COUNT="2", GIT_CONFIG_KEY_0="http.extraHeader",
                   GIT_CONFIG_VALUE_0=f"Authorization: token {token}",
                   GIT_CONFIG_KEY_1="credential.helper", GIT_CONFIG_VALUE_1="")
    if log:
        run_logged(["git", *argv], log, cwd=cwd, env=env)
        return None
    completed = subprocess.run(["git", *argv], cwd=cwd, env=env, capture_output=True,
                               text=True, timeout=180, check=False)
    if completed.returncode:
        raise RuntimeError("git command failed; use the retained stack logs")
    return completed.stdout.strip()


def stop_process(process, *, grace=30):
    """Stop an owned session leader, then contain remaining descendants."""
    if process is None:
        return
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=grace)
        except subprocess.TimeoutExpired:
            pass
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    process.wait(timeout=10)


def wait_ready(process, log, needles, timeout=90):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"stack process exited before readiness; inspect {log}")
        text = Path(log).read_text(errors="replace") if Path(log).exists() else ""
        if all(needle in text for needle in needles):
            return
        time.sleep(0.1)
    raise TimeoutError(f"stack readiness exceeded {timeout}s; inspect {log}")
