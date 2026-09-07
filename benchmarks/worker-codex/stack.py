"""Run one issue through an isolated real Forgejo, host runner and Temper."""

from contextlib import contextmanager
import json
import hashlib
import os
from pathlib import Path
import shutil
import socket
import subprocess
import time

from stack_config import configure
from stack_forge import Forge, await_delivery
from stack_sessions import read_agent_sessions
from stack_lifecycle import close_stack
from stack_support import git, private_json, run_logged, wait_ready


class Stack:
    """Owned local processes plus retained benchmark evidence paths."""

    def __init__(self, root, temper_bin, fixture_bin):
        self.root = Path(root).resolve()
        self.repo_owner = "benchmark-" + hashlib.sha256(str(self.root).encode()).hexdigest()[:12]
        self.repository = self.repo_owner + "/repo"
        self.temper_bin, self.fixture_bin = Path(temper_bin).resolve(), Path(fixture_bin).resolve()
        self.helper = self.process = None
        self.bootstrap = {}
        self.bundle = self.root / "bundle"
        self.seed_checkout = self.root / "seed"
        self.final_checkout = self.root / "final"
        self.journal_root = self.root / "state/agent-traces/journal"
        self.env = os.environ.copy()
        self.env.update(TEMPER_LOG_FORMAT="json", RUST_LOG="info",
                        XDG_CONFIG_HOME=str(self.root / "xdg-config"),
                        XDG_STATE_HOME=str(self.root / "xdg-state"))
        self.env["PATH"] = str(self.temper_bin.parent) + os.pathsep + self.env.get("PATH", "")
        self._files = []
        self._issue_created = False
        self._owned_root = False

    def boot(self, seed, auth_file, workflow, codebase_memory, env):
        if self.root.exists() and any(self.root.iterdir()):
            raise ValueError("stack root must be new or empty")
        self.root.mkdir(parents=True, exist_ok=True, mode=0o700)
        self.root.chmod(0o700)
        self._owned_root = True
        if env:
            self.env.update(env)
        if not Path(auth_file).is_file():
            raise FileNotFoundError("explicit OAuth auth_file does not exist")
        if not (Path(seed) / ".forgejo/workflows/ci.yml").is_file():
            raise FileNotFoundError("seed must contain .forgejo/workflows/ci.yml with the benchmark gate")
        setup_started_ns = time.monotonic_ns()
        self._boot_fixture()
        self._init(workflow)
        self.effective_configuration = configure(self.bundle, self.root, self.temper_bin,
                                                 auth_file, codebase_memory)
        self._seed(Path(seed))
        self._launch()
        self.setup_seconds = (self.ready_ns - setup_started_ns) / 1e9
        private_json(self.root / "stack.json", {
            "topology": "forgejo-host-runner-temper-standalone",
            "forgejo_version": self.bootstrap["forgejo_version"],
            "runner_version": self.bootstrap["runner_version"],
            "seed_sha": self.seed_sha, "seed_checkout": str(self.seed_checkout),
            "ready_monotonic_ns": self.ready_ns, "setup_seconds": self.setup_seconds,
            "effective_configuration": self.effective_configuration,
            "repository": self.repository, "repo_owner": self.repo_owner,
            "graph_identity": "unique logical repository owner and fresh workspace",
            "codebase_memory_cache": self.env.get("CBM_CACHE_DIR", "provider_default"),
        })

    def _log(self, name):
        handle = (self.root / name).open("ab")
        self._files.append(handle)
        return handle

    def _boot_fixture(self):
        self.helper = subprocess.Popen([str(self.fixture_bin), str(self.root / "fixture")],
                                       stdin=subprocess.PIPE, stdout=self._log("fixture-ready.log"),
                                       stderr=self._log("fixture.log"), env=self.env,
                                       start_new_session=True)
        wait_ready(self.helper, self.root / "fixture-ready.log", ['"ready":true'], 180)
        self.bootstrap = json.loads((self.root / "fixture/bootstrap.json").read_text())
        self.forge = Forge(self.bootstrap["base_url"], self.bootstrap["admin_token"], self.repository)

    def _init(self, workflow):
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        argv = [str(self.temper_bin), "--config", str(self.bundle), "init", "--non-interactive",
                "--force", "--apply", "--yes", "--forge", self.bootstrap["base_url"],
                "--repo", self.repository, "--bind", f"127.0.0.1:{port}",
                "--workspace", str(self.root / "workspaces"), "--admin-user", "benchmark-admin",
                "--provider", "chatgpt", "--workflow", str(workflow or "basic-delivery")]
        env = self.env | {"TEMPER_INIT_ADMIN_PASSWORD": self.bootstrap["admin_password"]}
        run_logged(argv, self.root / "init.log", env=env)

    def _seed(self, seed):
        shutil.copytree(seed, self.seed_checkout, ignore=shutil.ignore_patterns(".git", "target", "__pycache__"))
        log = self.root / "seed.log"
        for args in (["init", "-b", "main"], ["config", "user.name", "Benchmark Admin"],
                     ["config", "user.email", "benchmark-admin@example.invalid"],
                     ["add", "--all"], ["commit", "-m", "Frozen benchmark baseline"]):
            git(args, cwd=self.seed_checkout, log=log,
                extra_env={"GIT_AUTHOR_DATE": "2000-01-01T00:00:00Z",
                           "GIT_COMMITTER_DATE": "2000-01-01T00:00:00Z"})
        self.seed_sha = git(["rev-parse", "HEAD"], cwd=self.seed_checkout)
        self.remote = self.bootstrap["base_url"] + "/" + self.repository + ".git"
        git(["remote", "add", "origin", self.remote], cwd=self.seed_checkout, log=log)
        git(["push", "--set-upstream", "origin", "HEAD:main"], cwd=self.seed_checkout,
            token=self.bootstrap["admin_token"], log=log)
        self.forge.await_branch("main", self.seed_sha)

    def _launch(self):
        log = self._log("temper.log")
        self.process = subprocess.Popen([str(self.temper_bin), "--config", str(self.bundle),
                                         "serve", "standalone"], stdin=subprocess.DEVNULL,
                                        stdout=log, stderr=subprocess.STDOUT, env=self.env,
                                        start_new_session=True)
        wait_ready(self.process, self.root / "temper.log",
                   ["webhook listener up", "ready -- watching"], 120)
        self.ready_ns = time.monotonic_ns()

    def run(self, task, title="Implement the benchmark coding task", timeout_seconds=1800):
        if self._issue_created:
            raise RuntimeError("each stack accepts exactly one benchmark issue")
        self._issue_created = True
        start_ns = time.monotonic_ns()
        self.forge.deadline = time.monotonic() + timeout_seconds
        status = "failed"
        try:
            result = self._deliver(task, title, start_ns)
            status = "succeeded"
            return result
        except BaseException as error:
            sessions = None
            if (self.root / "temper.log").exists():
                try:
                    sessions = read_agent_sessions(self.root / "temper.log", require_complete=False)
                except ValueError:
                    pass
            private_json(self.root / "delivery.json", {
                "status": "failed", "error_type": type(error).__name__, "error": str(error),
                "coding_session_seconds": None, "session_timing_complete": False,
                "agent_sessions": sessions, "journal_root": str(self.journal_root),
            })
            raise
        finally:
            self.forge.deadline = None
            stopped_ns = time.monotonic_ns()
            private_json(self.root / "attempt.json", {
                "status": status, "start_monotonic_ns": start_ns, "stop_monotonic_ns": stopped_ns,
                "elapsed_seconds": (stopped_ns - start_ns) / 1e9,
            })

    def _deliver(self, task, title, start_ns):
        issue = self.forge.create_code_issue(title, task)
        filed_ns = time.monotonic_ns()
        private_json(self.root / "issue-created.json", {"issue_number": issue["number"],
                     "request_start_monotonic_ns": start_ns, "response_monotonic_ns": filed_ns})
        issue, pr, ci, first_pr_ns, finished_ns = await_delivery(self, issue["number"], self.forge.deadline)
        git(["clone", self.remote, str(self.final_checkout)], cwd=self.root,
            token=self.bootstrap["admin_token"], log=self.root / "final-checkout.log")
        final_sha = git(["rev-parse", "HEAD"], cwd=self.final_checkout)
        if final_sha == self.seed_sha or final_sha != pr.get("merge_commit_sha"):
            raise RuntimeError("final default branch does not match the merged implementation")
        sessions = read_agent_sessions(self.root / "temper.log")
        if not any(session["role"] == "engineer" for session in sessions):
            raise ValueError("merged benchmark has no observed engineer coding session")
        result = {"status": "succeeded", "session_timing_complete": True,
                  "issue_number": issue["number"], "pull_request_number": pr["number"],
                  "head_sha": pr["head"]["sha"], "merge_commit_sha": pr.get("merge_commit_sha"),
                  "final_sha": final_sha, "seed_sha": self.seed_sha,
                  "issue_created_at": issue["created_at"], "merged_at": pr.get("merged_at"),
                  "issue_request_monotonic_ns": start_ns, "issue_filed_monotonic_ns": filed_ns,
                  "first_pr_observed_monotonic_ns": first_pr_ns,
                  "landed_observed_monotonic_ns": finished_ns,
                  "issue_to_merge_observed_seconds": (finished_ns - start_ns) / 1e9,
                  "final_checkout": str(self.final_checkout), "journal_root": str(self.journal_root),
                  "ci_run_count": len(ci), "session_timing_source": "agent.finished.duration_ms",
                  "runtime_kind": "in_process", "agent_sessions": sessions,
                  "coding_session_seconds": sum(s["duration_ms"] for s in sessions
                                                if s["role"] == "engineer") / 1000,
                  "all_agent_session_seconds": sum(s["duration_ms"] for s in sessions) / 1000,
                  "setup_seconds": self.setup_seconds}
        private_json(self.root / "delivery.json", result)
        return result

    def close(self):
        close_stack(self)

@contextmanager
def start(*, seed, root, temper_bin, fixture_bin, auth_file, workflow=None,
          codebase_memory=None, env=None):
    """Provision/seed/start; no issue or model request occurs before `run`."""
    stack = Stack(root, temper_bin, fixture_bin)
    try:
        stack.boot(seed, auth_file, workflow, codebase_memory, env)
        yield stack
    finally:
        stack.close()
