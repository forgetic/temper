"""Deterministic campaign orchestration and artifact tests; no model or Cargo."""

from contextlib import ExitStack, nullcontext, redirect_stdout
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
from test_metrics import native_tools, successful_trials


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
SPEC = importlib.util.spec_from_file_location("campaign_under_test", ROOT / "campaign.py")
campaign = importlib.util.module_from_spec(SPEC)
# Never provision a native stack in these tests.
with patch.dict(sys.modules, {"stack": SimpleNamespace(start=None),
                              "stack_sessions": SimpleNamespace(read_agent_sessions=None)}):
    SPEC.loader.exec_module(campaign)


class CampaignTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="worker-codex-campaign-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        namespace = patch.object(campaign, "prepare_namespace", return_value={
            "namespace": "test-namespace", "complete": False})
        namespace.start()
        self.addCleanup(namespace.stop)

    def test_failure_is_retained_and_all_five_alternating_pairs_run(self):
        seen = []
        snapshots = []
        options = SimpleNamespace(output=self.root / "campaign", repository=self.root / "source",
                                  task_revision="frozen-revision", pairs=5, timeout_seconds=45)

        def export(_repository, _revision, output):
            (output / "fixture/repo").mkdir(parents=True)
            (output / "task.md").write_text("Frozen task\n")
            return output

        def arm(name, _seed, task, output, _inputs, _options):
            self.assertEqual(task, "Frozen task\n")
            seen.append(name)
            snapshots.append(json.loads((options.output / "campaign.json").read_text()))
            output.mkdir(parents=True, exist_ok=True)
            if len(seen) == 1:
                raise RuntimeError("deterministic first-attempt failure")
            trial = next(t for t in successful_trials(1) if t["contestant"] == name)
            trial["coding_seconds"] = 1 if name == "temper" else 2
            return trial

        configuration = {"schema_version": 1, "model": "gpt-6-astra", "reasoning_effort": "xhigh",
                         "codex_config_sha256": "frozen-configuration-hash"}
        with ExitStack() as mocks, redirect_stdout(io.StringIO()):
            mocks.enter_context(patch.dict("os.environ", {"TEMPER_BENCHMARK_LIVE": "1"}))
            mocks.enter_context(patch.object(campaign, "export_inputs", side_effect=export))
            mocks.enter_context(patch.object(campaign, "preflight", side_effect=lambda _: dict(configuration)))
            mocks.enter_context(patch.object(campaign, "seed_commit"))
            mocks.enter_context(patch.object(campaign, "git", return_value="a" * 40))
            mocks.enter_context(patch.object(campaign, "native_arm", side_effect=lambda *args: arm("temper", *args)))
            mocks.enter_context(patch.object(campaign, "codex_arm", side_effect=lambda *args: arm("codex", *args)))
            result = campaign.execute(options)

        expected = ["temper", "codex", "codex", "temper", "temper", "codex",
                    "codex", "temper", "temper", "codex"]
        self.assertEqual(seen, expected)
        self.assertEqual(len(result["trials"]), 10)
        self.assertFalse(result["performance_target_met"])
        self.assertEqual(result["contestants"]["temper"]["passed"], 4)
        self.assertNotIn("median_seconds", result["contestants"]["temper"])
        for index, trial in enumerate(result["trials"]):
            self.assertEqual((trial["pair"], trial["order"]), (index // 2 + 1, index % 2 + 1))
            self.assertGreaterEqual(trial["attempt_wall_seconds"], 0)
            retained = options.output / f"pairs/{trial['pair']:03}/{trial['contestant']}/trial.json"
            self.assertEqual(json.loads(retained.read_text()), trial)
        self.assertIn("deterministic first-attempt failure", result["trials"][0]["error"])
        self.assertTrue(all(snapshot == snapshots[0] for snapshot in snapshots))
        self.assertEqual(snapshots[0]["codex_config_sha256"], "frozen-configuration-hash")
        self.assertEqual(snapshots[0]["seed_sha"], "a" * 40)
        self.assertEqual(json.loads((options.output / "comparison.json").read_text()), result)

    def test_live_opt_in_is_checked_before_creating_output(self):
        options = SimpleNamespace(output=self.root / "campaign")
        with patch.dict("os.environ", {"TEMPER_BENCHMARK_LIVE": "0"}):
            with self.assertRaisesRegex(ValueError, "TEMPER_BENCHMARK_LIVE"):
                campaign.execute(options)
        self.assertFalse(options.output.exists())

    def test_configuration_drift_retains_failed_arm_without_running_it(self):
        options = SimpleNamespace(output=self.root / "campaign", repository=self.root / "source",
                                  task_revision="frozen-revision", pairs=1, timeout_seconds=45)

        def export(_repository, _revision, output):
            (output / "fixture/repo").mkdir(parents=True)
            (output / "task.md").write_text("Frozen task\n")
            return output

        def arm(_seed, _task, output, _inputs, _options):
            output.mkdir(parents=True)
            return successful_trials(1)[0]

        snapshots = [{"config_hash": "initial"}, {"config_hash": "initial"}, {"config_hash": "changed"}]
        with ExitStack() as mocks, redirect_stdout(io.StringIO()):
            mocks.enter_context(patch.dict("os.environ", {"TEMPER_BENCHMARK_LIVE": "1"}))
            mocks.enter_context(patch.object(campaign, "export_inputs", side_effect=export))
            mocks.enter_context(patch.object(campaign, "preflight", side_effect=snapshots))
            mocks.enter_context(patch.object(campaign, "seed_commit"))
            mocks.enter_context(patch.object(campaign, "git", return_value="a" * 40))
            native = mocks.enter_context(patch.object(campaign, "native_arm", side_effect=arm))
            codex = mocks.enter_context(patch.object(campaign, "codex_arm"))
            result = campaign.execute(options)
        native.assert_called_once()
        codex.assert_not_called()
        self.assertEqual(len(result["trials"]), 2)
        self.assertTrue(result["trials"][0]["correct"])
        self.assertFalse(result["trials"][1]["correct"])
        self.assertIn("changed during the campaign", result["trials"][1]["error"])
        self.assertEqual(json.loads((options.output / "campaign.json").read_text())["config_hash"], "initial")

    def test_comparison_rejects_invalid_durations_and_missing_tool_evidence(self):
        baseline = successful_trials()
        self.assertTrue(campaign.comparison(baseline, 5)["performance_target_met"])
        for field, value in [("coding_seconds", float("nan")), ("coding_seconds", float("inf")),
                             ("coding_seconds", -1), ("coding_seconds", 0),
                             ("coding_seconds", None), ("tool_calls", None), ("graph_calls", None)]:
            with self.subTest(field=field, value=value):
                trials = [dict(trial) for trial in baseline]
                trials[0][field] = value
                result = campaign.comparison(trials, 5)
                self.assertFalse(result["performance_target_met"])
                evidence_key = "complete" if field == "coding_seconds" else "tool_evidence_complete"
                self.assertFalse(result["contestants"]["temper"][evidence_key])

    def test_incomplete_native_analysis_preserves_clock_and_readable_trace_metrics(self):
        for failure in ["exit", "missing", "json", "schema"]:
            with self.subTest(failure=failure):
                trial, arm, calls = self.native_trial(failure=failure)
                self.assertEqual(len(calls), 2)
                self.assertEqual(trial["coding_seconds"], 4)
                self.assertEqual(len(trial["agent_sessions"]), 2)
                self.assertFalse(trial["agent_succeeded"])
                self.assertFalse(trial["correct"])
                self.assertFalse(trial["tool_evidence_complete"])
                self.assertIsNone(trial["tool_calls"])
                self.assertIsNone(trial["tokens"])
                self.assertEqual(trial["mcp"], {"complete": True, "provider_calls": 2})
                analysis = trial["analysis"]
                self.assertFalse(analysis["complete"])
                self.assertEqual((analysis["traces"], analysis["analyzed_traces"]), (2, 1))
                self.assertEqual(analysis["invocations"], 2)
                self.assertEqual(analysis["observed_metrics"]["tool_calls"], 3)
                self.assertEqual(analysis["observed_metrics"]["graph_calls"], 1)
                self.assertEqual(len(analysis["errors"]), 1)
                self.assertIn("analyzer raw output", Path(analysis["errors"][0]["log"]).read_text())
                self.assertTrue((arm / "analysis-1.log").exists())
                self.assertTrue(trial["validation"]["passed"])

    def test_native_validation_exception_preserves_complete_agent_metrics(self):
        trial, arm, _calls = self.native_trial(validation_error=ValueError("broken oracle report"))
        self.assertEqual(trial["coding_seconds"], 4)
        self.assertEqual(trial["tool_calls"], 6)
        self.assertEqual(trial["graph_calls"], 2)
        self.assertEqual(trial["mcp"]["provider_calls"], 2)
        self.assertTrue(trial["analysis"]["complete"])
        self.assertTrue(trial["agent_succeeded"])
        self.assertFalse(trial["correct"])
        self.assertFalse(trial["validation"]["complete"])
        self.assertIn("broken oracle report", trial["validation"]["error"])
        self.assertEqual(json.loads((arm / "validation.json").read_text()), trial["validation"])

    def test_codex_validation_timeout_preserves_run_metrics_and_completed_gate(self):
        arm = self.root / "codex"
        options = SimpleNamespace(codex_bin=Path("codex"), mcp_bin=Path("mcp"), timeout_seconds=30)

        def run_codex(_checkout, _task, output, **_kwargs):
            output.mkdir()
            events = [{"type": "thread.started", "thread_id": "retained-session"},
                      {"type": "item.started", "item": {"id": "1", "type": "command_execution"}},
                      {"type": "item.completed", "item": {"id": "1", "type": "command_execution",
                                                             "exit_code": 0}},
                      {"type": "turn.completed", "usage": {"input_tokens": 100,
                                                             "cached_input_tokens": 40,
                                                             "output_tokens": 5}}]
            (output / "events.jsonl").write_text("".join(json.dumps({"event": event,
                "elapsed_seconds": index}) + "\n" for index, event in enumerate(events)))
            return {"process_wall_seconds": 7.5, "exit_code": 0, "timed_out": False}

        def run(command, **kwargs):
            if command[0] == "git":
                (arm / "repo").mkdir()
            else:
                kwargs["stdout"].write(b"retained host output\n")
                if command[1] == "test":
                    raise subprocess.TimeoutExpired(command, kwargs["timeout"])
            return subprocess.CompletedProcess(command, 0)

        with ExitStack() as mocks:
            mocks.enter_context(patch.object(campaign, "run_codex", side_effect=run_codex))
            mocks.enter_context(patch.object(campaign.subprocess, "run", side_effect=run))
            mocks.enter_context(patch.object(campaign, "mcp_metrics",
                                            return_value={"complete": True, "provider_calls": 2}))
            trial = campaign.codex_arm(self.root / "seed", "task", arm, self.root / "inputs", options)
        self.assertEqual(trial["coding_seconds"], 7.5)
        self.assertEqual(trial["session_id"], "retained-session")
        self.assertEqual(trial["tool_calls"], 1)
        self.assertEqual(trial["tokens"]["total_input_tokens"], 100)
        self.assertEqual(trial["tokens"]["cached_input_tokens"], 40)
        self.assertEqual(trial["mcp"]["provider_calls"], 2)
        self.assertTrue(trial["agent_succeeded"])
        self.assertFalse(trial["correct"])
        validation = trial["validation"]
        self.assertFalse(validation["complete"])
        self.assertIn("TimeoutExpired", validation["error"])
        self.assertEqual([gate["complete"] for gate in validation["gates"]], [True, False])
        self.assertTrue(validation["gates"][0]["passed"])
        self.assertTrue(all(gate["duration_seconds"] >= 0 for gate in validation["gates"]))
        self.assertIn("retained host output", (arm / "host-tests.log").read_text())
        self.assertEqual(json.loads((arm / "validation.json").read_text()), validation)

    def test_oracle_timeout_retains_completed_gates_and_raw_oracle_log(self):
        output = self.root / "artifacts"
        output.mkdir()
        trial = {"agent_succeeded": True, "coding_seconds": 12, "tool_calls": 7}

        def run(command, **kwargs):
            kwargs["stdout"].write(b"raw validation output\n")
            if command[0] == sys.executable:
                raise subprocess.TimeoutExpired(command, kwargs["timeout"])
            return subprocess.CompletedProcess(command, 0)

        with patch.object(campaign.subprocess, "run", side_effect=run):
            campaign.validate_trial(trial, self.root / "repo", self.root / "seed",
                                    self.root / "inputs", output)
        self.assertEqual(trial["coding_seconds"], 12)
        self.assertEqual(trial["tool_calls"], 7)
        self.assertFalse(trial["correct"])
        self.assertFalse(trial["validation"]["complete"])
        self.assertEqual(len(trial["validation"]["gates"]), 2)
        self.assertTrue(all(gate["passed"] for gate in trial["validation"]["gates"]))
        self.assertGreaterEqual(trial["validation"]["oracle_wall_seconds"], 0)
        self.assertIn("raw validation output", (output / "host-oracle.log").read_text())

    def test_native_model_mismatch_cannot_pass_with_successful_delivery_and_oracle(self):
        trial, _arm, _calls = self.native_trial(model="gpt-5.5")
        self.assertEqual(trial["coding_seconds"], 4)
        self.assertTrue(trial["validation"]["passed"])
        self.assertFalse(trial["agent_succeeded"])
        self.assertFalse(trial["correct"])
        self.assertFalse(trial["model_evidence"]["matches_requested_model"])

    def native_trial(self, *, failure=None, validation_error=None, model="gpt-6-astra"):
        arm = self.root / (failure or "complete")
        journal = arm / "journals"
        for name in ["first", "second"]:
            (journal / name).mkdir(parents=True)
            (journal / name / "manifest.json").write_text("{}")
            (journal / name / "events.jsonl").write_text(json.dumps({"event": {
                "type": "model.call.started", "data": {"provider": "openai-codex", "model": model}}}) + "\n")
        summary = {"terminal": {"status": "succeeded"},
                   "trace": {"terminal_event_observed": True, "events": {"observed": 10, "expected": 10}},
                   "metrics": {"model": {"calls": 1, "attempts": 1},
                      "tools": native_tools({"shell": 2, "codebase_memory_search_graph": 1})}}
        calls = []

        def analyze(command, **kwargs):
            calls.append(command)
            kwargs["stdout"].write(b"analyzer raw output\n")
            output = Path(command[command.index("--output-dir") + 1])
            output.mkdir(parents=True)
            if len(calls) == 1 and failure:
                if failure == "exit":
                    raise subprocess.CalledProcessError(1, command)
                if failure != "missing":
                    (output / "run.json").write_text("{" if failure == "json" else "[]")
            else:
                (output / "run.json").write_text(json.dumps(summary))
            return subprocess.CompletedProcess(command, 0)

        stack = SimpleNamespace(seed_sha="a" * 40, journal_root=journal,
                                run=lambda *_args, **_kwargs: {"final_checkout": str(arm / "repo")})
        options = SimpleNamespace(temper_bin=Path("temper"), fixture_bin=Path("fixture"),
                                  auth_file=Path("auth"), analyzer_bin=Path("analyzer"),
                                  mcp_bin=Path("mcp"), timeout_seconds=30)
        sessions = [{"duration_ms": 1250, "status": "failed"},
                    {"duration_ms": 2750, "status": "succeeded"}]
        with ExitStack() as mocks:
            mocks.enter_context(patch.object(campaign, "start", return_value=nullcontext(stack)))
            mocks.enter_context(patch.object(campaign, "git", return_value="a" * 40))
            mocks.enter_context(patch.object(campaign, "read_agent_sessions", return_value=sessions))
            mocks.enter_context(patch.object(campaign.subprocess, "run", side_effect=analyze))
            mocks.enter_context(patch.object(campaign, "mcp_metrics",
                                            return_value={"complete": True, "provider_calls": 2}))
            mocks.enter_context(patch.object(campaign, "validate", side_effect=validation_error,
                                            return_value={"passed": True}))
            trial = campaign.native_arm(self.root / "seed", "task", arm, self.root / "inputs", options)
        return trial, arm, calls

    def test_export_uses_frozen_revision_instead_of_dirty_working_tree(self):
        repository = self.root / "source"
        task = repository / "benchmarks/worker-codex/task.md"
        task.parent.mkdir(parents=True)
        task.write_text("frozen task\n")
        (task.parent / "seed.txt").write_text("frozen seed\n")
        baseline = self.initialize(repository)
        task.write_text("uncommitted modified task\n")
        exported = campaign.export_inputs(repository, baseline, self.root / "export")
        self.assertEqual((exported / "task.md").read_text(), "frozen task\n")
        self.assertEqual((exported / "seed.txt").read_text(), "frozen seed\n")

    def test_patch_contains_untracked_staged_and_unstaged_changes_without_changing_index(self):
        repository = self.root / "candidate"
        repository.mkdir()
        (repository / "tracked.txt").write_text("baseline\n")
        (repository / ".gitignore").write_text("/target/\n")
        baseline = self.initialize(repository)
        (repository / "tracked.txt").write_text("staged\n")
        self.git(repository, "add", "tracked.txt")
        (repository / "tracked.txt").write_text("final unstaged\n")
        (repository / "new.rs").write_text("pub fn answer() -> u32 { 42 }\n")
        (repository / "target").mkdir()
        (repository / "target/ignored").write_text("build artifact\n")
        before_index = (repository / ".git/index").read_bytes()
        before_status = self.git(repository, "status", "--porcelain")
        output = self.root / "artifacts"
        output.mkdir()
        campaign.capture_patch(repository, baseline, output)
        diff = (output / "candidate.patch").read_text()
        self.assertIn("diff --git a/new.rs b/new.rs", diff)
        self.assertIn("+pub fn answer() -> u32 { 42 }", diff)
        self.assertIn("+final unstaged", diff)
        self.assertNotIn("build artifact", diff)
        self.assertEqual((repository / ".git/index").read_bytes(), before_index)
        self.assertEqual(self.git(repository, "status", "--porcelain"), before_status)
        self.assertFalse((output / "snapshot.index").exists())

    def initialize(self, repository):
        self.git(repository, "init", "--quiet", "-b", "main")
        self.git(repository, "add", "--all")
        self.git(repository, "-c", "user.name=Benchmark Test", "-c", "user.email=test@example.invalid",
                 "-c", "commit.gpgsign=false", "commit", "--quiet", "-m", "baseline")
        return self.git(repository, "rev-parse", "HEAD").strip()

    @staticmethod
    def git(repository, *args):
        return subprocess.check_output(["git", "-C", str(repository), *args], text=True)


if __name__ == "__main__":
    unittest.main()
