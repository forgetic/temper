"""Always attempt every owned shutdown and scrub ephemeral credentials."""

from pathlib import Path
import shutil

from stack_sessions import read_agent_sessions
from stack_support import private_json, stop_process


def close_stack(stack):
    if not stack._owned_root or getattr(stack, "_closed", False):
        return
    stack._closed = True
    errors = []

    def attempt(label, action):
        try:
            return action()
        except Exception as error:
            errors.append(f"{label}: {type(error).__name__}: {error}")
            return None

    attempt("stop Temper", lambda: stop_process(stack.process, grace=45))
    helper = stack.helper
    if helper:
        if helper.stdin:
            attempt("close fixture stdin", helper.stdin.close)
        try:
            helper.wait(timeout=30)
        except Exception as error:
            errors.append(f"fixture graceful wait: {type(error).__name__}: {error}")
            attempt("stop fixture", lambda: stop_process(helper, grace=3))
        if helper.returncode not in (None, 0):
            errors.append(f"fixture exited {helper.returncode}; inspect fixture.log")
        if helper.returncode is not None:
            for key in ("server_data_dir", "runner_work_dir"):
                path = Path(stack.bootstrap[key]) if key in stack.bootstrap else None
                if path and path.exists():
                    attempt(f"remove owned {key}", lambda path=path: shutil.rmtree(path))
    for handle in stack._files:
        attempt("close log", handle.close)
    for path in (stack.bundle / "credentials.toml", stack.bundle / "webhook-secret",
                 stack.root / "fixture/bootstrap.json"):
        attempt("remove ephemeral credential", lambda path=path: path.unlink(missing_ok=True))
    stack.bootstrap.clear()
    if (stack.root / "temper.log").exists():
        sessions = attempt("read agent boundary evidence", lambda: read_agent_sessions(
            stack.root / "temper.log", require_complete=False))
        attempt("retain agent boundaries", lambda: private_json(stack.root / "agent-sessions.json", sessions))
    attempt("retain cleanup evidence", lambda: private_json(stack.root / "cleanup.json", {"errors": errors}))
    if errors:
        raise RuntimeError("stack cleanup/evidence errors: " + "; ".join(errors))
