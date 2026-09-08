"""Provider evidence stays distinct from model attempts and incomplete capture."""

import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from metrics import mcp_metrics


class McpMetricsTests(unittest.TestCase):
    def test_ids_are_scoped_to_process_and_request_types(self):
        rows = []
        for session in ["first", "second"]:
            rows.append(self.row(session, "process_start"))
            for identifier, outcome in [(1, "success"), ("1", "tool_error")]:
                fields = {"method": "tools/call", "id": identifier,
                          "tool_name": "search_graph", "arguments_sha256": "same"}
                rows.append(self.row(session, "request", direction="client_to_provider", **fields))
                rows.append(self.row(session, "response", request_direction="client_to_provider",
                                     outcome=outcome, duration_seconds=0.25, **fields))
            rows.append(self.row(session, "process_exit", exit_code=0))
        result = self.measure(rows)
        self.assertTrue(result["complete"])
        self.assertEqual(result["provider_calls"], 4)
        self.assertEqual(result["outcomes"], {"success": 2, "tool_error": 2})
        self.assertEqual(result["duration_seconds"], 1)
        self.assertEqual(result["repeated_identical_requests"], 3)

    def test_missing_response_or_process_exit_prevents_complete_evidence(self):
        start = self.row("s", "process_start")
        call = self.row("s", "request", direction="client_to_provider", method="tools/call",
                        id=2, tool_name="index_repository", arguments_sha256="cold")
        for rows in [[start], [start, call, self.row("s", "process_exit", exit_code=0)]]:
            self.assertFalse(self.measure(rows)["complete"])
        result = self.measure([start, call])
        self.assertEqual(result["index_calls"], 1)
        self.assertEqual(result["outcomes"], {"missing": 1})

    def test_transport_diagnostic_is_retained_and_invalidates_capture(self):
        result = self.measure([self.row("s", "process_start"),
                               self.row("s", "transport", reason="message_too_large"),
                               self.row("s", "process_exit", exit_code=0)])
        self.assertFalse(result["complete"])
        self.assertEqual(result["diagnostics"], ["message_too_large"])
        with tempfile.TemporaryDirectory() as temporary:
            absent = mcp_metrics(Path(temporary) / "missing.jsonl")
        self.assertFalse(absent["available"])
        self.assertIsNone(absent["provider_calls"])

    @staticmethod
    def row(session, event, **fields):
        return {"proxy_session_id": session, "event": event, **fields}

    @staticmethod
    def measure(rows):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "mcp.jsonl"
            path.write_text("".join(json.dumps(row) + "\n" for row in rows))
            return mcp_metrics(path)


if __name__ == "__main__":
    unittest.main()
