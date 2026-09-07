"""Evidence accounting must not manufacture parity from missing/failed data."""

import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from metrics import codex_metrics, comparison, temper_metrics


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
        self.assertEqual(result["tool_calls"], 4)
        self.assertEqual(result["tokens"]["total_input_tokens"], 200)
        self.assertEqual(result["tokens"]["uncached_input_tokens"], 120)
        self.assertIsNone(temper_metrics(runs)["coding_seconds"])

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
        self.assertEqual(result["temper_to_codex_ratio"], 0.5)
        self.assertEqual(result["contestants"]["temper"]["min_seconds"], 5)
        self.assertEqual(result["contestants"]["temper"]["passed"], 5)

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
                "trace": {"terminal_event_observed": True},
                "metrics": {"tools": {"failed": 0, "by_name": {
                    "read": {"calls": 1}, "codebase_memory_search_graph": {"calls": 1}}},
                    "tokens": {"input_tokens": 60, "cache_read_tokens": 40,
                               "coverage": {"observed": 1, "expected": 1}}}}

    @staticmethod
    def trials():
        return [{"contestant": name, "pair": pair, "correct": True,
                 "agent_succeeded": True, "coding_seconds": seconds,
                 "tool_calls": 4, "graph_calls": 2, "mcp": {"complete": True}}
                for pair in range(1, 6) for name, seconds in [("temper", 5), ("codex", 10)]]


if __name__ == "__main__":
    unittest.main()
