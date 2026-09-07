"""Product behavior and the exact report delivered by the engineer."""
import copy
import json
from pathlib import Path
import unittest

from report_summary import artifact_recommendation
from verify_report import verify


class ReportSummaryTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.report = json.loads(Path("graph-artifact-report.json").read_text())

    def test_delivered_report_has_complete_verified_contract(self):
        verify(self.report)
        self.assertEqual(artifact_recommendation(self.report), "defer")

    def test_synthetic_decision_cannot_claim_native_adoption(self):
        report = copy.deepcopy(self.report)
        report["decision"] = "bounded-ci-pilot-candidate"
        report["native_performance_evidence"] = True
        self.assertEqual(artifact_recommendation(report), "defer")

    def test_native_candidate_needs_all_accounting_and_correctness(self):
        report = copy.deepcopy(self.report)
        report.update(evidence_kind="native", native_performance_evidence=True,
                      decision="bounded-ci-pilot-candidate")
        self.assertEqual(artifact_recommendation(report), "bounded-ci-pilot-candidate")
        for field in ("complete_cell_accounting", "failure_case_accounting", "correctness_passed"):
            incomplete = {**report, field: False}
            self.assertEqual(artifact_recommendation(incomplete), "defer")
        self.assertEqual(artifact_recommendation({**report, "default_persistence_enabled": True}), "defer")

    def test_incomplete_and_unknown_reports_defer(self):
        self.assertEqual(artifact_recommendation({}), "defer")
        self.assertEqual(artifact_recommendation({"decision": "adopt"}), "defer")


if __name__ == "__main__":
    unittest.main()
