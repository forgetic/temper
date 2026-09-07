"""Bind deterministic harness checks to the checkout that completed live delivery."""

import json
from pathlib import Path
import re
import subprocess
import sys
import unittest


SUITES = ("test_metrics.py", "test_mcp_metrics.py", "test_mcp_proxy.py", "test_campaign.py")


def require(condition, message):
    if not condition:
        raise ValueError(message)


def check_context(context, head):
    evidence = context["run_evidence"]
    scenario = evidence["scenario"]
    expected = {"name": "worker-codex-benchmark", "feature": "ai/temper#1285",
                "plan": "ai/temper#1285", "source_branch": "agent/pr-for-feature-1285",
                "runner_id": "manifest", "tier": "live", "checkout_head_sha": head}
    require(all(scenario.get(key) == value for key, value in expected.items()),
            "live evidence must identify this mapped feature checkout")
    topology = {"kind": "single-repo-forgejo-standalone", "forge": "forgejo",
                "runner": "forgejo-actions-host", "temper": "standalone",
                "agent_model": "scripted-fake-llm"}
    require(scenario.get("topology") == topology, "the required real live topology is missing")
    require(re.fullmatch(r"sha256:[0-9a-f]{64}", scenario.get("resolved_content_digest", "")),
            "resolved scenario identity is missing")
    require(evidence.get("verdict") == "passed" and evidence.get("assertions", {}).get("status") == "passed",
            "the live manifest and its required assertions must already pass")
    final = evidence["final_state"]
    pulls = final.get("pull_requests", [])
    issues = final.get("issues", [])
    require(len(pulls) == 1 and pulls[0].get("merged_sha") == context.get("merged_sha")
            and re.fullmatch(r"[0-9a-f]{40,64}", context.get("merged_sha") or ""),
            "one actual merged implementation PR is required")
    require(len(issues) == 1 and issues[0].get("state") == "closed", "the source issue must close")
    require(final.get("ci", {}).get("completed_jobs", 0) > 0, "real host CI completion is required")
    require(evidence.get("provider", {}).get("request_count", 0) > 0, "Jig model turns must be observed")


def exact_sources(repository, head):
    benchmark = repository / "benchmarks/worker-codex"
    scenario = repository / "scenarios/worker-codex-benchmark"
    sources = list(benchmark.glob("*.py")) + list((benchmark / "tests").glob("*.py"))
    sources += [benchmark / "README.md", benchmark / "task.md"]
    sources += [path for path in scenario.rglob("*") if path.is_file() and "__pycache__" not in path.parts]
    require(all((benchmark / "tests" / name).is_file() for name in SUITES), "the real benchmark test suites are missing")
    for source in sources:
        relative = source.relative_to(repository).as_posix()
        frozen = subprocess.check_output(["git", "-C", str(repository), "show", f"{head}:{relative}"])
        require(source.read_bytes() == frozen, "scenario and benchmark sources must match the tracked feature head")
    return benchmark


def run_suites(benchmark):
    sys.path.insert(0, str(benchmark))
    counts = {}
    for filename in SUITES:
        suite = unittest.defaultTestLoader.discover(str(benchmark / "tests"), pattern=filename)
        require(suite.countTestCases() > 0, "a required deterministic suite discovered no tests")
        result = unittest.TextTestRunner(stream=sys.stderr, verbosity=2).run(suite)
        require(result.wasSuccessful() and not result.skipped, "a required deterministic suite failed or skipped")
        counts[filename] = result.testsRun
    return counts


def main():
    require(len(sys.argv) == 2, "one assertion context path is required")
    context = json.loads(Path(sys.argv[1]).read_text())
    repository = Path(subprocess.check_output(["git", "rev-parse", "--show-toplevel"], text=True).strip())
    head = subprocess.check_output(["git", "-C", str(repository), "rev-parse", "HEAD"], text=True).strip()
    check_context(context, head)
    benchmark = exact_sources(repository, head)
    counts = run_suites(benchmark)
    report = {
        "schema_version": 1,
        "evidence_kind": "deterministic_harness_checks_after_live_jig_delivery",
        "real_model_performance_measured": False,
        "real_provider_usage_measured": False,
        "performance_target_met": False,
        "test_counts": counts,
        "checkpoints": ["tracked_feature_sources_verified", "real_host_ci_completed",
                        "one_pr_merged", "source_issue_closed", "paired_accounting_tests_passed",
                        "provider_accounting_tests_passed", "stdio_proxy_tests_passed",
                        "campaign_retention_tests_passed"],
    }
    output = Path(context["artifact_directory"]) / "deterministic-harness-checks.json"
    output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, sort_keys=True))


if __name__ == "__main__":
    main()
