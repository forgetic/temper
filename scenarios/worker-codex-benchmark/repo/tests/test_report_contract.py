import unittest

from report_contract import EXPECTED, validate


class ReportContractTests(unittest.TestCase):
    def test_accepts_only_the_scripted_delivery_declaration(self):
        validate(dict(EXPECTED))

    def test_rejects_timing_provider_and_performance_claims(self):
        for field in ("real_model_performance_measured", "real_provider_usage_measured",
                      "performance_target_met"):
            with self.subTest(field=field):
                with self.assertRaises(ValueError):
                    validate(dict(EXPECTED) | {field: True})

    def test_rejects_extra_metrics_missing_fields_and_ambiguous_types(self):
        invalid = [dict(EXPECTED) | {"coding_seconds": 1},
                   {key: value for key, value in EXPECTED.items() if key != "evidence_kind"},
                   dict(EXPECTED) | {"performance_target_met": 0},
                   dict(EXPECTED) | {"schema_version": True}, []]
        for report in invalid:
            with self.subTest(report=report):
                with self.assertRaises(ValueError):
                    validate(report)


if __name__ == "__main__":
    unittest.main()
