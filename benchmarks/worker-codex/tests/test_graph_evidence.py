"""Graph eligibility must not turn fallback or a prior graph into matched evidence."""

from copy import deepcopy
import hashlib
import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from graph_evidence import codex_graph_evidence, native_graph_evidence
from metrics import comparison
from test_metrics import successful_trials


NAMESPACE = "worker-codex-" + "a" * 32


def namespace_proof():
    instructions = "Use " + NAMESPACE
    return {"namespace": NAMESPACE, "complete": True, "missing_before_start": True,
            "source": "host_index_status_before_codex", "status_is_error": True,
            "status_request": {"name": "index_status", "arguments": {"project": NAMESPACE}},
            "status_error": "project not found or not indexed", "instructions": instructions,
            "instructions_sha256": hashlib.sha256(instructions.encode()).hexdigest()}


class GraphEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.checkout = self.root / "checkout"
        self.checkout.mkdir()

    def test_codex_uses_fresh_current_index_and_all_scoped_reads(self):
        events = self.index() + self.call("list", "list_projects", {}) + self.read()
        result = self.codex(events)
        self.assertTrue(result["eligible"])
        self.assertEqual(result["project"], NAMESPACE)
        self.assertEqual(result["scoped_reads"], 1)
        # A reindex after editing is allowed within the same fresh namespace.
        self.assertTrue(self.codex(events + self.index("reindex"))["eligible"])

    def test_codex_rejects_cross_project_and_unscoped_reads(self):
        for project in ["prior-attempt", None, "temper-current"]:
            with self.subTest(project=project):
                result = self.codex(self.index() + self.read(project=project))
                self.assertFalse(result["eligible"])
                self.assertIn("graph_read_outside_current_project", result["violations"])

    def test_codex_checks_read_start_before_index_completion(self):
        index, read = self.index(), self.read()
        for events in [read + index, [index[0], read[0], index[1], read[1]]]:
            result = self.codex(events)
            self.assertFalse(result["eligible"])
            self.assertIn("graph_read_before_current_index", result["violations"])

    def test_codex_rejects_wrong_root_namespace_or_unready_result(self):
        for mutation in ["root", "requested_namespace", "returned_namespace", "not_ready", "failed"]:
            with self.subTest(mutation=mutation):
                events = self.index() + self.read()
                if mutation == "root":
                    for event in events[:2]:event["item"]["arguments"]["repo_path"] = str(self.root / "old")
                elif mutation == "requested_namespace":
                    for event in events[:2]:event["item"]["arguments"]["name"] = "old"
                elif mutation == "returned_namespace":
                    events[1]["item"]["result"]["structured_content"]["project"] = "old"
                elif mutation == "not_ready":
                    events[1]["item"]["result"]["structured_content"]["status"] = "indexing"
                else:
                    events[1]["item"]["status"] = "failed"
                self.assertFalse(self.codex(events)["eligible"])

    def test_missing_proof_or_malformed_lifecycle_never_claims_eligibility(self):
        for proof in [None, {}, {**namespace_proof(), "missing_before_start": False},
                      {**namespace_proof(), "status_request": {"name": "index_status", "arguments": {"project": "other"}}},
                      {**namespace_proof(), "instructions_sha256": "different"}]:
            self.assertFalse(self.codex(self.index() + self.read(), proof=proof)["eligible"])
        for events in [[], self.index(), self.index() + self.read()[:1],
                       self.index()[1:] + self.read(),
                       self.index() + [{"type": "capture.invalid_json"}],
                       self.index() + self.call("unknown", "new_unscoped_search", {})]:
            self.assertFalse(self.codex(events)["eligible"])
        path = self.root / "malformed.jsonl"
        path.write_text("{broken\n")
        self.assertFalse(codex_graph_evidence(path, self.checkout, namespace_proof())["complete"])

    def test_native_explicit_systemic_fallback_remains_ineligible(self):
        for category in ["project_not_ready", "circuit_open"]:
            result = self.native(failure={"category": category, "reason": category,
                                         "fallback_to_conventional_discovery": True})
            self.assertTrue(result["complete"])
            self.assertFalse(result["eligible"])
            self.assertTrue(result["systemic_fallback_observed"])
            self.assertEqual(result["fallbacks"][0]["category"], category)

    def test_native_local_miss_and_success_are_not_systemic_fallback(self):
        for failure in [None, {"category": "candidate_miss", "reason": "not_found",
                               "fallback_to_conventional_discovery": False}]:
            result = self.native(failure=failure)
            self.assertTrue(result["eligible"])
            self.assertFalse(result["systemic_fallback_observed"])

    def test_native_missing_trace_completion_or_typed_failure_fails_closed(self):
        for mutation in ["sequence", "completion", "trace_count", "call_count", "failure"]:
            with self.subTest(mutation=mutation):
                self.assertFalse(self.native(mutation=mutation)["eligible"])

    def test_graph_ineligibility_preserves_correctness_and_descriptive_timing(self):
        for contestant in ["temper", "codex"]:
            for graph in [None, {"complete": False, "eligible": False},
                          {"complete": True, "eligible": False}]:
                trials = successful_trials()
                trial = next(t for t in trials if t["contestant"] == contestant)
                trial["graph_evidence"] = graph
                result = comparison(trials)
                self.assertTrue(trial["correct"])
                self.assertTrue(result["timing_target_met"])
                self.assertEqual(result["contestants"][contestant]["passed"], 5)
                self.assertFalse(result["contestants"][contestant]["graph_eligible"])
                self.assertFalse(result["performance_target_met"])

    def index(self, identifier="index"):
        return self.call(identifier, "index_repository", {"repo_path": str(self.checkout), "name": NAMESPACE},
                         {"status": "indexed", "project": NAMESPACE})

    def read(self, project=NAMESPACE):
        args = {"name_pattern": ".*Policy.*"}
        if project is not None:args["project"] = project
        return self.call("read", "search_graph", args, {"total": 1})

    @staticmethod
    def call(identifier, tool, args, result=None):
        item = {"id": identifier, "type": "mcp_tool_call", "server": "codebase-memory-mcp",
                "tool": tool, "arguments": args, "status": "in_progress", "result": None, "error": None}
        completed = {**deepcopy(item), "status": "completed",
                     "result": {"structured_content": result or {}}}
        return [{"type": "item.started", "item": item}, {"type": "item.completed", "item": completed}]

    def codex(self, events, *, proof="default"):
        path = self.root / "events.jsonl"
        path.write_text("".join(json.dumps({"event": event, "elapsed_seconds": n}) + "\n"
                                for n, event in enumerate(events)))
        return codex_graph_evidence(path, self.checkout, namespace_proof() if proof == "default" else proof)

    def native(self, *, failure=None, mutation=None):
        root = self.root / "journal"
        root.mkdir(exist_ok=True)
        events = [{"type": "run.started", "data": {}},
                  {"type": "tool.started", "data": {"call_id": "1", "name": "codebase_memory_search_graph"}},
                  {"type": "tool.finished", "data": {"call_id": "1", "name": "codebase_memory_search_graph",
                      "status": "failed" if failure else "succeeded", "failure": failure}},
                  {"type": "run.finished", "data": {}}]
        if mutation == "completion":events.pop(2)
        if mutation == "failure":events[2]["data"].update(status="failed", failure={"category": "unknown"})
        rows = [{"seq": n + 1, "event": event} for n, event in enumerate(events)]
        if mutation == "sequence":rows[1]["seq"] = 7
        (root / "events.jsonl").write_text("".join(json.dumps(row) + "\n" for row in rows))
        return native_graph_evidence(root, 2 if mutation == "trace_count" else 1,
                                     2 if mutation == "call_count" else 1)


if __name__ == "__main__":
    unittest.main()
