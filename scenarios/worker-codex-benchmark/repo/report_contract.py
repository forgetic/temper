"""Validate a scripted delivery declaration without manufacturing timing evidence."""

import json
from pathlib import Path
import sys


EXPECTED = {
    "schema_version": 1,
    "feature": "ai/temper#1285",
    "evidence_kind": "jig_delivery_contract",
    "real_model_performance_measured": False,
    "real_provider_usage_measured": False,
    "performance_target_met": False,
    "benchmark_task": "benchmarks/worker-codex/task.md",
    "independent_harness_checks": "required_after_convergence",
}


def validate(report):
    if not isinstance(report, dict) or set(report) != set(EXPECTED):
        raise ValueError("report must contain only the declared contract fields")
    if any(type(report[key]) is not type(value) or report[key] != value
           for key, value in EXPECTED.items()):
        raise ValueError("scripted delivery must not claim measured performance or provider usage")


def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: report_contract.py REPORT.json")
    validate(json.loads(Path(sys.argv[1]).read_text()))
    print("Scripted delivery contract accepted; real model performance remains unmeasured.")


if __name__ == "__main__":
    main()
