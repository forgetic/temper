"""Requested configuration must never certify a differently selected model."""

import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from native_model import model_evidence
from metrics import temper_metrics


class NativeModelTests(unittest.TestCase):
    def test_every_attempt_must_match_including_retries(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            path = root / "events.jsonl"
            good = {"event": {"type": "model.call.started", "data": {
                "provider": "openai-codex", "model": "gpt-6-astra"}}}
            wrong = {"event": {"type": "model.call.started", "data": {
                "provider": "openai-codex", "model": "gpt-5.5"}}}
            for events, expected, matches in [([good, good], 2, True),
                                              ([good, wrong], 2, False),
                                              ([good], 2, False), ([], 0, False)]:
                path.write_text("".join(json.dumps(e) + "\n" for e in events))
                result = model_evidence(root, expected)
                self.assertEqual(result["matches_requested_model"], matches)
                self.assertIsNone(result["provider_reported_model"])
            path.write_text(json.dumps(good) + "\nnot JSON\n")
            result = model_evidence(root, 1)
            self.assertFalse(result["complete"])
            self.assertTrue(result["errors"])

    def test_failed_provider_without_usage_has_unknown_tokens(self):
        summary = {"metrics": {"tokens": {"input_tokens": 0, "cache_read_tokens": 0,
                                           "coverage": {"observed": 0, "expected": 0}}}}
        self.assertIsNone(temper_metrics([summary])["tokens"])


if __name__ == "__main__":
    unittest.main()
