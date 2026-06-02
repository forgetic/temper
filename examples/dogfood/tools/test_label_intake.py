from __future__ import annotations

import unittest

import label_intake


class LabelIntakeTests(unittest.TestCase):
    def test_would_label_new_unlabeled_issue(self) -> None:
        started_at = label_intake.parse_time("2026-06-02T12:00:00Z")
        issue = {
            "number": 42,
            "created_at": "2026-06-02T12:01:00Z",
            "labels": [],
        }

        decision = label_intake.intake_label_decision(issue, started_at, seen=set())

        self.assertEqual(label_intake.DECISION_LABEL, decision)

    def test_skips_product_issue(self) -> None:
        started_at = label_intake.parse_time("2026-06-02T12:00:00Z")
        issue = {
            "number": 42,
            "created_at": "2026-06-02T12:01:00Z",
            "labels": [{"name": "product"}],
        }

        decision = label_intake.intake_label_decision(issue, started_at, seen=set())

        self.assertEqual(label_intake.DECISION_SKIP, decision)


if __name__ == "__main__":
    unittest.main()
