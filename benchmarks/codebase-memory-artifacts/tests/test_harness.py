"""Non-native tests: failures, privacy, deterministic fixture and accounting."""
import copy
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from artifact import validate_artifact
from contract import CELLS, Refusal, coverage_gate, digest, local_source, verify_current
from fixture import FixtureClient, fixture_artifact, fixture_sources, run_fixture
from reporting import build_report, project_sample
from native import NativeSession
from source import checkout, git
from protocol import McpClient
from accounting import AccountMeter


class HarnessTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.root = Path(self.directory.name)

    def tearDown(self):
        self.directory.cleanup()

    def test_fixture_is_repeatable_private_and_never_performance(self):
        first = run_fixture(self.root)
        second_dir = self.root / "second"
        second_dir.mkdir()
        second = run_fixture(second_dir)
        self.assertEqual(first, second)
        self.assertEqual(len(first["samples"]), 25)
        self.assertEqual(len(first["failures"]), 4)
        self.assertTrue(first["correctness_passed"])
        self.assertEqual(first["decision"], "defer")
        self.assertFalse(first["native_performance_evidence"])
        for cell in CELLS:
            self.assertEqual(sorted(s["repetition"] for s in first["samples"] if s["cell"] == cell), [1, 2, 3, 4, 5])
            self.assertIsNone(first["summary"][cell]["ready_correct_median_seconds"])
        encoded = json.dumps(first)
        self.assertNotIn(str(self.root), encoded)
        self.assertNotIn("def probe", encoded)
        self.assertNotIn("/synthetic/producer", encoded)
        self.assertTrue(all(first["facts"].values()))
        refusals = {s["cell"]: s["initial_refusal"] for s in first["failures"]}
        self.assertEqual(refusals, {"missing": "artifact-missing", "corrupt": "artifact-checksum-mismatch",
                                    "truncated": "artifact-size-mismatch", "incompatible": "artifact-incompatible-schema"})

    def test_exact_source_rejects_wrong_root_bytes_and_range(self):
        roots, queries = fixture_sources(self.root)
        expected = queries["p0"]["symbols"][0]
        client = FixtureClient(roots["p0"], "fixture", queries["p0"])
        snippet = client.call("get_code_snippet", {"qualified_name": "fixture.probe"})
        local_source(roots["p0"], expected, snippet)
        for key, value in (("file_path", str(roots["p1"] / "probe.py")), ("source", "foreign source"), ("end_line", 99)):
            changed = {**snippet, key: value}
            with self.assertRaises(Refusal):
                local_source(roots["p0"], expected, changed)

    def test_foreign_root_fails_before_any_snippet_is_read(self):
        roots, queries = fixture_sources(self.root)
        client = FixtureClient(roots["p0"], "fixture", queries["p0"])
        with self.assertRaisesRegex(Refusal, "current-root-mismatch"):
            verify_current(client, roots["p1"], "fixture", queries["p1"])
        self.assertEqual(client.calls, [{"tool": "index_status"}])

    def test_coverage_unavailable_is_not_valid_empty(self):
        roots, queries = fixture_sources(self.root)
        client = FixtureClient(roots["p0"], "fixture", queries["p0"])
        result = client.call("check_index_coverage", {})
        for key, value in (("generation", "stale"), ("generation_matches", False), ("hash_records_complete", False),
                           ("recording_status", "unavailable")):
            changed = copy.deepcopy(result)
            changed["metadata"][key] = value
            with self.assertRaises(Refusal):
                coverage_gate(changed, "fixture", queries["p0"]["coverage_paths"])

    def test_missing_failure_or_duplicate_repetition_disqualifies_report(self):
        fixture = run_fixture(self.root)
        config = {"provider": fixture["provider"], "source": fixture["source"],
                  "experiment": {"repetitions": 5, "minimum_improvement_fraction": .2}}
        rows = copy.deepcopy(fixture["samples"])
        for row in rows:
            row["initial_refusal"] = "none"
            row["metrics"]["ready_correct_seconds"] = 10.0 if row["cell"].startswith("cold") else 1.0
            row["metrics"]["consumer_job_seconds"] = row["metrics"]["ready_correct_seconds"]
        passed = build_report(config, rows, fixture["failures"], {}, True, "a" * 64)
        self.assertEqual(passed["decision"], "bounded-ci-pilot-candidate")
        missing = build_report(config, rows, fixture["failures"][:-1], {}, True, "a" * 64)
        self.assertEqual(missing["decision"], "defer")
        rows[5]["repetition"] = 1
        duplicate = build_report(config, rows, fixture["failures"], {}, True, "a" * 64)
        self.assertFalse(duplicate["complete_cell_accounting"])

    def test_failed_observation_is_retained_and_has_no_success_only_median(self):
        fixture = run_fixture(self.root)
        config = {"provider": fixture["provider"], "source": fixture["source"],
                  "experiment": {"repetitions": 5, "minimum_improvement_fraction": .2}}
        rows = fixture["samples"]
        rows[0]["ready_correct"] = False
        result = build_report(config, rows, fixture["failures"], {}, True, None)
        self.assertEqual(len(result["samples"]), 25)
        self.assertIsNone(result["summary"]["cold-p0"]["ready_correct_median_seconds"])
        self.assertEqual(result["decision"], "defer")

    def test_projection_discards_private_diagnostics_and_untrusted_reason(self):
        sample = {"cell": "cold-p0", "repetition": 1, "source_commit": "0" * 40, "daemon_start": "cold",
                  "initial_refusal": "/private/operator/token", "raw": "SECRET", "metrics": {"path": "/private"}}
        result = project_sample(sample)
        self.assertEqual(result["initial_refusal"], "internal-error")
        self.assertNotIn("private", json.dumps(result))

    def test_manifest_rejects_build_source_config_ignore_and_download_changes(self):
        report = run_fixture(self.root)
        config = {"provider": report["provider"], "source": report["source"], "project": "synthetic-artifact-fixture",
                  "experiment": {"mode": "full", "provider_environment": {}}}
        manifest = report["producer"]["manifest"]
        artifact = self.root / "producer-artifact"
        self.assertTrue(validate_artifact(artifact, config, manifest))
        for area, key in (("provider", "sha256"), ("source", "p0")):
            changed = copy.deepcopy(config)
            changed[area][key] = "bad"
            with self.assertRaisesRegex(Refusal, "artifact-identity-mismatch"):
                validate_artifact(artifact, changed, manifest)
        changed = copy.deepcopy(config)
        changed["experiment"]["mode"] = "fast"
        with self.assertRaisesRegex(Refusal, "artifact-identity-mismatch"):
            validate_artifact(artifact, changed, manifest)
        changed = copy.deepcopy(config)
        changed["source"]["ignore_sha256"]["p0"] = "bad"
        with self.assertRaisesRegex(Refusal, "artifact-identity-mismatch"):
            validate_artifact(artifact, changed, manifest)

    def test_failed_client_initialization_retains_cpu_and_forced_cleanup(self):
        class FailingClient:
            def __init__(self, *args):
                self.forced_cleanup, self.calls, self.startup_seconds = True, [], .5
            def initialize(self):
                raise Refusal("provider-rpc-error")
            def close(self):
                pass
        experiment = SimpleNamespace(uid=62078, gid=62078, cache=self.root, raw=self.root,
                                     run={"sample_interval_seconds": .02, "timeout_seconds": 1, "shutdown_timeout_seconds": 1},
                                     config={"provider": {"path": "/never-executed"}}, environment={})
        with patch("native.McpClient", FailingClient), patch("native.AccountMeter") as mocked:
            mocked.return_value.finish.return_value = {"natural_account_exit": True, "cpu_accounting_consistent": True,
                                                       "user_cpu_seconds": .75, "system_cpu_seconds": .25}
            mocked.return_value.seen = {}
            mocked.return_value.worker_logs = {}
            with self.assertRaises(Refusal) as caught:
                NativeSession(experiment, self.root, "project", "failed")
            measured = caught.exception.session_measurements
            self.assertFalse(measured["natural_account_exit"])
            self.assertEqual(measured["user_cpu_seconds"], .75)
            self.assertEqual(measured["provider_startup_seconds"], .5)

    def test_checkout_disables_ambient_git_config_and_uses_independent_objects(self):
        repository = self.root / "repository"
        subprocess.run(["git", "init", "-q", str(repository)], check=True)
        (repository / "probe.rs").write_text("fn probe() {}\n")
        git(repository, "add", "probe.rs")
        git(repository, "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "-qm", "fixture")
        commit = git(repository, "rev-parse", "HEAD").decode().strip()
        poison = self.root / "poison-config"
        poison.write_text("[core]\n excludesFile = /must-not-be-used\n")
        destination = self.root / "clone"
        with patch.dict(os.environ, {"GIT_CONFIG_GLOBAL": str(poison)}):
            checkout(repository, commit, destination, os.getuid(), os.getgid())
        self.assertEqual(git(destination, "config", "--get", "core.excludesFile").strip(), b"/dev/null")
        self.assertEqual((destination / ".git/info/exclude").read_bytes(), b"")
        for source_object in (repository / ".git/objects").glob("*/*"):
            cloned = destination / ".git/objects" / source_object.relative_to(repository / ".git/objects")
            if cloned.is_file():
                self.assertNotEqual((source_object.stat().st_dev, source_object.stat().st_ino),
                                    (cloned.stat().st_dev, cloned.stat().st_ino))

    def test_broken_stdin_close_still_waits_and_closes_output_streams(self):
        client = McpClient.__new__(McpClient)
        client.closed, client.forced_cleanup = False, False
        client.process, client.stderr = Mock(), Mock()
        client.process.stdin.close.side_effect = BrokenPipeError()
        client.close()
        client.process.wait.assert_called_once_with(timeout=15)
        client.process.stdout.close.assert_called_once()
        client.stderr.close.assert_called_once()
        self.assertFalse(client.forced_cleanup)

    def test_frontend_timeout_retains_forced_cleanup_fact(self):
        client = McpClient.__new__(McpClient)
        client.closed, client.forced_cleanup = False, False
        client.process, client.stderr = Mock(), Mock()
        client.process.wait.side_effect = [subprocess.TimeoutExpired("fixture", 15), 0]
        client.close()
        client.process.terminate.assert_called_once()
        self.assertTrue(client.forced_cleanup)

    def test_process_roles_survive_exec_zombie_and_pid_reuse(self):
        meter = AccountMeter.__new__(AccountMeter)
        meter.seen, meter.peak, meter.samples = {}, 0, 0
        def snapshot(start, arguments, state="S", rss=4096):
            return {"start": start, "arguments": arguments, "command": " ".join(arguments), "state": state, "rss": rss}
        meter.observe({17: snapshot(100, ["provider", "--tool-profile=analysis"])})
        meter.observe({17: snapshot(100, ["provider", "cli", "--index-worker", "--index-worker-build", "build"])})
        meter.observe({17: snapshot(100, [], "Z", 0)})
        worker = meter.seen[(17, 100)]
        self.assertTrue(worker["observed_index_worker"])
        self.assertIn("--index-worker", worker["command"])
        self.assertEqual((worker["state"], worker["rss"]), ("Z", 0))
        meter.observe({17: snapshot(200, ["provider", "--tool-profile=analysis"])})
        self.assertEqual(len(meter.seen), 2)
        self.assertFalse(meter.seen[(17, 200)]["observed_index_worker"])
        self.assertTrue(meter.seen[(17, 100)]["observed_index_worker"])
        self.assertEqual(meter.peak, 4096)

    def test_daemon_role_uses_exact_audited_argument_and_survives_empty_snapshot(self):
        meter = AccountMeter.__new__(AccountMeter)
        meter.seen, meter.peak, meter.samples = {}, 0, 0
        for arguments in (["provider", "--cbm-daemon-internal"], []):
            meter.observe({19: {"start": 300, "arguments": arguments, "command": " ".join(arguments), "rss": 0}})
        self.assertTrue(meter.seen[(19, 300)]["observed_daemon"])
        self.assertEqual(meter.seen[(19, 300)]["command"], "provider --cbm-daemon-internal")
        arguments = ["/some/--index-worker/path", "--index-worker-build", "--cbm-daemon-internal-extra"]
        meter.observe({20: {"start": 400, "arguments": arguments, "command": " ".join(arguments), "rss": 0}})
        self.assertFalse(meter.seen[(20, 400)]["observed_daemon"])
        self.assertFalse(meter.seen[(20, 400)]["observed_index_worker"])


if __name__ == "__main__":
    unittest.main()
