"""Evidence accounting must not manufacture parity from missing/failed data."""

from copy import deepcopy
import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from metrics import codex_metrics, comparison, temper_metrics


def native_tools(counts):
    def group(calls):
        return {"calls": calls, "succeeded": calls, "failed": 0, "cancelled": 0,
                "duration_coverage": {"observed": calls, "expected": calls}}
    return {**group(sum(counts.values())),
            "by_name": {name: group(calls) for name, calls in counts.items()}}


def successful_trials(pairs=5):
    trials = []
    for pair in range(1, pairs + 1):
        order = ("temper", "codex") if pair % 2 else ("codex", "temper")
        for ordinal, name in enumerate(order, 1):
            trial = {"contestant": name, "pair": pair, "order": ordinal, "correct": True,
                     "agent_succeeded": True, "coding_seconds": 5 if name == "temper" else 10,
                     "validation": {"complete": True, "passed": True},
                     "configuration_evidence": {
                         phase: {"complete": True, "matches_frozen": True}
                         for phase in ["before", "after"]},
                     "graph_evidence": {"complete": True, "eligible": True},
                     "tool_calls": 4, "graph_calls": 2, "tool_evidence_complete": True,
                     "mcp": {"available": True, "complete": True, "provider_calls": 3}}
            if name == "temper":
                trial.update(attempts=1, model_attempts=2,
                    analysis={"complete": True, "errors": [], "traces": 1,
                              "analyzed_traces": 1, "invocations": 1},
                    delivery={"status": "succeeded", "merged_at": "2026-09-07T01:00:00Z",
                              "final_sha": "b" * 40, "merge_commit_sha": "b" * 40,
                              "seed_sha": "a" * 40, "ci_run_count": 1,
                              "session_timing_complete": True},
                    model_evidence={"complete": True, "matches_requested_model": True,
                                    "observed_attempts": 2, "expected_attempts": 2, "errors": [],
                                    "reasoning_effort": "xhigh", "provider_reported_model": None,
                                    "requests": [{"provider": "openai-codex", "model": "gpt-6-astra",
                                                  "attempts": 2}]})
            trials.append(trial)
    return trials


class MetricsTests(unittest.TestCase):
    def test_codex_deduplicates_item_updates_and_keeps_model_calls_unknown(self):
        call = {"id": "tool1", "type": "mcp_tool_call", "server": "codebase-memory-mcp",
                "tool": "search_graph", "status": "in_progress"}
        events = [
            {"type": "thread.started", "thread_id": "s1"},
            {"type": "item.started", "item": call},
            {"type": "item.updated", "item": call},
            {"type": "item.completed", "item": {**call, "status": "completed"}},
            {"type": "turn.completed", "usage": {"input_tokens": 100,
                "cached_input_tokens": 40, "output_tokens": 10}},
        ]
        result = self.codex(events)
        self.assertTrue(result["agent_succeeded"])
        self.assertEqual(result["tool_calls"], 1)
        self.assertEqual(result["graph_by_name"], {"search_graph": 1})
        self.assertIsNone(result["model_calls"])
        self.assertEqual(result["tokens"]["total_input_tokens"], 100)
        self.assertEqual(result["tokens"]["uncached_input_tokens"], 60)

    def test_missing_tool_completion_or_invalid_json_is_not_success(self):
        for event in [
            {"type": "item.started", "item": {"id": "x", "type": "command_execution"}},
            {"type": "capture.invalid_json"},
        ]:
            result = self.codex([event, {"type": "turn.completed", "usage": {}}])
            self.assertFalse(result["agent_succeeded"])

    def test_native_counts_failed_attempts_and_uses_invocation_timer(self):
        runs = [self.native("failed"), self.native("succeeded")]
        result = temper_metrics(runs, [{"duration_ms": 4000, "status": "failed"},
                                      {"duration_ms": 6000, "status": "succeeded"}])
        self.assertEqual(result["coding_seconds"], 10)
        self.assertEqual(result["attempts"], 2)
        self.assertTrue(result["agent_succeeded"])
        self.assertTrue(result["tool_evidence_complete"])
        self.assertEqual(result["tool_calls"], 4)
        self.assertEqual(result["tokens"]["total_input_tokens"], 200)
        self.assertEqual(result["tokens"]["uncached_input_tokens"], 120)
        self.assertIsNone(temper_metrics(runs)["coding_seconds"])

    def test_native_incomplete_trace_or_tool_accounting_cannot_pass(self):
        baseline = self.native("succeeded")
        mutations = [
            (("trace", "events", "observed"), 9),
            (("trace", "events"), None),
            (("trace", "events", "expected"), True),
            (("trace", "terminal_event_observed"), False),
            (("metrics", "tools", "duration_coverage", "observed"), 1),
            (("metrics", "tools", "succeeded"), 1),
            (("metrics", "tools", "calls"), 3),
            (("metrics", "tools", "by_name", "read", "duration_coverage"), None),
            (("metrics", "tools", "by_name", "read", "cancelled"), 1),
            (("metrics", "tools", "by_name", "read", "calls"), True),
        ]
        for keys, value in mutations:
            with self.subTest(keys=keys, value=value):
                run = deepcopy(baseline)
                target = run
                for key in keys[:-1]:
                    target = target[key]
                target[keys[-1]] = value
                result = temper_metrics([run], [{"duration_ms": 1000, "status": "succeeded"}])
                self.assertEqual(result["coding_seconds"], 1)
                self.assertFalse(result["tool_evidence_complete"])
                self.assertFalse(result["agent_succeeded"])

    def test_native_invalid_invocation_duration_stays_unknown(self):
        for value in [None, True, -1, 0, float("nan"), float("inf")]:
            with self.subTest(value=value):
                result = temper_metrics([self.native("succeeded")],
                                        [{"duration_ms": value, "status": "succeeded"}])
                self.assertIsNone(result["coding_seconds"])

    def test_failed_or_missing_pair_has_no_success_only_median(self):
        trials = self.trials()
        trials[0]["correct"] = False
        failed = comparison(trials)
        self.assertFalse(failed["performance_target_met"])
        self.assertNotIn("median_seconds", failed["contestants"]["temper"])
        missing = comparison(self.trials()[:-1])
        self.assertFalse(missing["performance_target_met"])
        duplicate = self.trials()
        duplicate[-1]["pair"] = 4
        self.assertFalse(comparison(duplicate)["performance_target_met"])

    def test_complete_pairs_report_ratio_and_variability(self):
        result = comparison(self.trials())
        self.assertTrue(result["performance_target_met"])
        self.assertTrue(result["order_complete"])
        self.assertEqual(result["temper_to_codex_ratio"], 0.5)
        self.assertEqual(result["contestants"]["temper"]["min_seconds"], 5)
        self.assertEqual(result["contestants"]["temper"]["passed"], 5)

    def test_at_least_five_pairs_are_required_even_with_complete_success(self):
        for pairs in [1, 4, 5, 6]:
            with self.subTest(pairs=pairs):
                result = comparison(successful_trials(pairs), pairs)
                self.assertEqual(result["performance_target_met"], pairs >= 5)
        for pairs in [0, -1, True, 5.0]:
            with self.subTest(pairs=pairs), self.assertRaises(ValueError):
                comparison([], pairs)

    def test_non_alternating_or_mislabeled_execution_order_cannot_pass(self):
        repeated_order = self.trials()
        repeated_order.sort(key=lambda t: (t["pair"], t["contestant"] != "temper"))
        for index, trial in enumerate(repeated_order):
            trial["order"] = index % 2 + 1
        extra = self.trials() + [{"contestant": "unknown", "pair": 6, "order": 1}]
        cases = [repeated_order, list(reversed(self.trials())), extra]
        for key, value in [("order", None), ("order", True), ("pair", True), ("contestant", "other")]:
            trials = self.trials()
            trials[0][key] = value
            cases.append(trials)
        for trials in cases:
            with self.subTest(first=trials[0]):
                result = comparison(trials)
                self.assertFalse(result["order_complete"])
                self.assertFalse(result["timing_target_met"])
                self.assertFalse(result["performance_target_met"])

    def test_native_requires_landed_delivery_and_every_requested_model(self):
        mutations = [
            (("analysis",), None), (("analysis", "complete"), False),
            (("analysis", "analyzed_traces"), 0), (("attempts",), 2),
            (("delivery",), None), (("delivery", "status"), "failed"),
            (("delivery", "merged_at"), None), (("delivery", "merge_commit_sha"), "c" * 40),
            (("delivery", "seed_sha"), "b" * 40), (("delivery", "ci_run_count"), 0),
            (("delivery", "session_timing_complete"), False),
            (("model_evidence",), None), (("model_evidence", "complete"), False),
            (("model_evidence", "matches_requested_model"), False),
            (("model_evidence", "expected_attempts"), 1),
            (("model_evidence", "observed_attempts"), 1),
            (("model_evidence", "reasoning_effort"), "high"),
            (("model_evidence", "errors"), ["missing request"]),
            (("model_evidence", "requests"), []),
            (("model_evidence", "requests", 0, "model"), "gpt-5.5"),
            (("model_evidence", "requests", 0, "provider"), "other"),
            (("model_evidence", "requests", 0, "attempts"), 1),
            (("model_attempts",), None),
        ]
        for keys, value in mutations:
            with self.subTest(keys=keys, value=value):
                trials = self.trials()
                target = trials[0]
                for key in keys[:-1]:
                    target = target[key]
                target[keys[-1]] = value
                result = comparison(trials)
                self.assertFalse(result["contestants"]["temper"]["complete"])
                self.assertFalse(result["performance_target_met"])

    def test_validation_and_tool_evidence_must_be_explicit_for_both_contestants(self):
        for index in [0, 1]:
            for key, value in [("validation", None), ("validation", {"passed": True}),
                               ("validation", {"passed": False, "complete": True}),
                               ("tool_evidence_complete", None), ("tool_evidence_complete", False),
                               ("tool_calls", None), ("graph_calls", True), ("graph_calls", 5),
                               ("mcp", None), ("mcp", {"complete": True}),
                               ("mcp", {"available": True, "complete": False, "provider_calls": 1})]:
                with self.subTest(contestant=index, key=key, value=value):
                    trials = self.trials()
                    trials[index][key] = value
                    self.assertFalse(comparison(trials)["performance_target_met"])

    def test_configuration_requires_matching_checks_before_and_after_each_arm(self):
        for index in [0, 1]:
            for phase in ["before", "after"]:
                for check in [None, {}, {"matches_frozen": True},
                              {"complete": True, "matches_frozen": False},
                              {"complete": False, "matches_frozen": True},
                              {"complete": True, "matches_frozen": 1}]:
                    with self.subTest(contestant=index, phase=phase, check=check):
                        trials = self.trials()
                        trials[index]["configuration_evidence"][phase] = check
                        result = comparison(trials)
                        cell = result["contestants"][trials[index]["contestant"]]
                        self.assertFalse(cell["configuration_complete"])
                        self.assertFalse(result["timing_target_met"])
                        self.assertFalse(result["performance_target_met"])
                        self.assertTrue(trials[index]["correct"])
            trials = self.trials()
            del trials[index]["configuration_evidence"]
            self.assertFalse(comparison(trials)["performance_target_met"])

    def codex(self, events):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "events.jsonl"
            path.write_text("".join(json.dumps({"event": event, "elapsed_seconds": i})
                                    + "\n" for i, event in enumerate(events)))
            return codex_metrics(path, {"process_wall_seconds": 8, "exit_code": 0,
                                       "timed_out": False})

    @staticmethod
    def native(status):
        return {"terminal": {"status": status}, "wall_time_ms": 1,
                "trace": {"terminal_event_observed": True, "events": {"observed": 10, "expected": 10}},
                "metrics": {"tools": native_tools({"read": 1, "codebase_memory_search_graph": 1}),
                    "tokens": {"input_tokens": 60, "cache_read_tokens": 40,
                               "coverage": {"observed": 1, "expected": 1}}}}

    @staticmethod
    def trials():
        return successful_trials()


if __name__ == "__main__":
    unittest.main()
