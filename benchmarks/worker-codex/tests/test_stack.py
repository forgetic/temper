"""Failure-sensitive checks for the benchmark stack, requiring no model."""

import json
from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib
import unittest
from unittest.mock import Mock, patch
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from stack import Stack, start
from stack_forge import Forge, ci_passed
from stack_sessions import read_agent_sessions
from stack_support import stop_process, write_toml


class StackTests(unittest.TestCase):
    def test_rejected_nonempty_root_does_not_remove_existing_credentials(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            credentials = root / "bundle/credentials.toml"
            credentials.parent.mkdir()
            credentials.write_text("untouched")
            with self.assertRaises(ValueError):
                with start(seed=root, root=root, temper_bin="temper", fixture_bin="fixture",
                           auth_file=root / "auth.json"):
                    self.fail("nonempty root accepted")
            self.assertEqual(credentials.read_text(), "untouched")

    def test_shutdown_failure_still_attempts_fixture_and_scrubs_credentials(self):
        with tempfile.TemporaryDirectory() as directory:
            stack = Stack(directory, "temper", "fixture")
            stack._owned_root = True
            stack.bundle.mkdir()
            secret = stack.bundle / "credentials.toml"
            secret.write_text("private")
            stack.process = object()
            stack.helper = Mock(returncode=0)
            stack.helper.stdin.close.side_effect = BrokenPipeError("already stopped")
            with patch("stack_lifecycle.stop_process", side_effect=OSError("stop failed")):
                with self.assertRaises(RuntimeError):
                    stack.close()
            stack.helper.wait.assert_called_once()
            self.assertFalse(secret.exists())
            self.assertTrue(json.loads((stack.root / "cleanup.json").read_text())["errors"])

    def test_failed_issue_filing_retains_attempt_and_unavailable_coding_time(self):
        with tempfile.TemporaryDirectory() as directory:
            stack = Stack(directory, "temper", "fixture")
            stack.forge = SimpleNamespace(deadline=None, create_code_issue=Mock(side_effect=TimeoutError("timeout")))
            with self.assertRaises(TimeoutError):
                stack.run("task", timeout_seconds=1)
            delivery = json.loads((stack.root / "delivery.json").read_text())
            attempt = json.loads((stack.root / "attempt.json").read_text())
            self.assertEqual(delivery["status"], "failed")
            self.assertIsNone(delivery["coding_session_seconds"])
            self.assertGreaterEqual(attempt["stop_monotonic_ns"], attempt["start_monotonic_ns"])
            self.assertIsNone(stack.forge.deadline)

    def test_forgejo_16_ci_evidence_uses_commit_sha(self):
        forge = Forge("http://local", "token")
        forge.repo = Mock(return_value={"workflow_runs": [
            {"id": 1, "commit_sha": "expected", "status": "success"},
            {"id": 2, "commit_sha": "other", "status": "success"},
        ]})
        self.assertEqual([run["id"] for run in forge.ci_evidence("expected")], [1])

    def test_other_native_roles_remain_visible_in_session_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "log.jsonl"
            base = {"artifact.ref": "repo PR#2", "role": "ci_diagnostician", "kind": "review"}
            events = [{"timestamp": "start", "fields": base | {"event": "agent.started"}},
                      {"timestamp": "end", "fields": base | {"event": "agent.finished",
                                                               "status": "succeeded", "duration_ms": 71}}]
            path.write_text("\n".join(map(json.dumps, events)))
            self.assertEqual(read_agent_sessions(path)[0]["role"], "ci_diagnostician")

    def test_native_project_identity_is_distinct_per_stack_without_cache_override(self):
        first = Stack("/tmp/benchmark-trial-1", "temper", "fixture")
        second = Stack("/tmp/benchmark-trial-2", "temper", "fixture")
        self.assertNotEqual(first.repo_owner, second.repo_owner)
        self.assertTrue(first.repository.endswith("/repo"))
        self.assertEqual(first.repo_owner, Stack(first.root, "temper", "fixture").repo_owner)
        self.assertNotEqual(first.env.get("CBM_CACHE_DIR"), str(first.root / "codebase-memory-cache"))

    def test_toml_round_trip_preserves_profile_and_array_of_pools(self):
        data = {"schema_version": 1, "worker": {"pools": [{"name": "engineers", "roles": ["engineer"]}]},
                "agent": {"profiles": {"engineers": {"command": ["/path with space/temper", "agent"],
                                                      "subagents": False}}}}
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "config.toml"
            write_toml(path, data)
            self.assertEqual(tomllib.loads(path.read_text()), data)
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)

    def test_owned_process_is_reaped(self):
        process = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"],
                                   start_new_session=True)
        stop_process(process, grace=1)
        self.assertIsNotNone(process.returncode)

    def test_ci_checks_latest_attempt_of_each_workflow(self):
        old = {"id": 1, "workflow_id": 1, "status": "failure"}
        fixed = {"id": 2, "workflow_id": 1, "status": "success"}
        other = {"id": 3, "workflow_id": 2, "status": "running"}
        self.assertTrue(ci_passed([old, fixed]))
        self.assertFalse(ci_passed([old, fixed, other]))
        self.assertFalse(ci_passed([]))

    def test_session_durations_include_failed_attempts_and_reject_missing(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "log.jsonl"
            events = []
            for status in ("failed", "succeeded"):
                base = {"artifact.ref": "repo#1", "role": "engineer", "kind": "coding"}
                events.extend([{"timestamp": "start", "fields": base | {"event": "agent.started"}},
                               {"timestamp": "end", "fields": base | {"event": "agent.finished",
                                                                        "status": status, "duration_ms": 42}}])
            path.write_text("\n".join(map(json.dumps, events)))
            self.assertEqual([s["status"] for s in read_agent_sessions(path)], ["failed", "succeeded"])
            del events[-1]["fields"]["duration_ms"]
            path.write_text("\n".join(map(json.dumps, events)))
            with self.assertRaises(ValueError):
                read_agent_sessions(path)


if __name__ == "__main__":
    unittest.main()
