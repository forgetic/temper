"""Validate only the explicit synthetic contract, never infer native performance."""
import hashlib
import json
import re
from pathlib import Path
import sys

CELLS = ("cold-p0", "artifact-p0", "cold-p1", "stale-artifact-p1", "warm-incremental-p1")
FAILURES = {"missing": "artifact-missing", "corrupt": "artifact-checksum-mismatch",
            "truncated": "artifact-size-mismatch", "incompatible": "artifact-incompatible-schema"}
CORRECT = {"current_root_correct", "source_correct", "symbols_correct", "calls_correct",
           "coverage_confirmed", "negative_checks_correct"}
MODULES = {"benchmark.py", "contract.py", "source.py", "artifact.py", "protocol.py",
           "accounting.py", "native.py", "reporting.py", "fixture.py"}
FACT_PREFIX = "artifact-benchmark-fact "


def require(condition, reason):
    if not condition:
        raise AssertionError(reason)


def exact_keys(value, keys):
    require(isinstance(value, dict) and set(value) == set(keys), "closed report object shape")


def digest(value, length=64):
    require(isinstance(value, str) and re.fullmatch("[0-9a-f]{" + str(length) + "}", value), "pinned digest")


def verify_harness():
    root = Path("benchmarks/codebase-memory-artifacts")
    expected = json.loads(Path(".artifact-harness-sha256.json").read_text())
    exact_keys(expected, MODULES)
    require({p.name for p in root.glob("*.py")} == MODULES, "exact embedded module set")
    for name, checksum in expected.items():
        digest(checksum)
        require(hashlib.sha256((root / name).read_bytes()).hexdigest() == checksum, "embedded harness bytes")


def verify_sample(sample, failure=False):
    keys = {"cell", "repetition", "source_commit", "daemon_start", "ready_correct", "recovered",
            "initial_refusal", "terminal_refusal", "natural_account_exit", "artifact_accepted", "correctness", "metrics"}
    # Native public-provider failure observations are deliberately absent here.
    if "provider_failure_probe" in sample:
        require(sample["provider_failure_probe"] is None, "synthetic native probe")
        keys.add("provider_failure_probe")
    exact_keys(sample, keys)
    require(sample["daemon_start"] == "synthetic", "synthetic daemon state")
    require(sample["ready_correct"] is True and sample["natural_account_exit"] is True, "ready and correct sample")
    require(sample["terminal_refusal"] == "none", "successful fallback terminal state")
    revision = "p0" if sample["cell"].endswith("p0") else "p1"
    require(sample["source_commit"] == ("0" if revision == "p0" else "1") * 40, "controlled source revision")
    correct = sample["correctness"]
    exact_keys(correct, CORRECT | {"generation", "coverage_signal"})
    require(all(correct[key] is True for key in CORRECT), "source symbol call coverage and deletion checks")
    require(correct["generation"] == "synthetic-generation", "bounded synthetic generation")
    require(correct["coverage_signal"] == "best_effort", "coverage does not claim exhaustive completeness")
    metrics = sample["metrics"]
    allowed = {"ready_correct_seconds", "consumer_job_seconds", "checkout_seconds", "transfer_seconds", "transfer_bytes",
               "index_request_count", "provider_request_count", "initial_index_seconds", "fallback_seconds",
               "provider_startup_seconds", "local_database_bytes", "retained_cache_bytes", "setup_seconds",
               "user_cpu_seconds", "system_cpu_seconds", "peak_concurrent_provider_rss_bytes", "rss_sample_count",
               "sample_interval_seconds", "account_job_seconds", "observed_process_count", "observed_daemon_count",
               "observed_index_worker_count", "warm_preparation_seconds"}
    exact_keys(metrics, allowed)
    for name, value in metrics.items():
        if name in {"index_request_count", "provider_request_count"}:
            require(type(value) is int and value > 0, "observed synthetic request count")
        else:
            require(value is None, "synthetic metrics remain unmeasured")
    artifact = sample["cell"] in {"artifact-p0", "stale-artifact-p1"}
    expected = FAILURES[sample["cell"]] if failure else "current-root-mismatch" if artifact else "none"
    require(sample["initial_refusal"] == expected, "closed artifact failure classification")
    require(sample["recovered"] is (expected != "none"), "observed recovery accounting")
    require(sample["artifact_accepted"] is artifact, "artifact acceptance precedes foreign-root refusal")


def verify_manifest(manifest):
    exact_keys(manifest, {"manifest_version", "source_commit", "provider_version", "provider_sha256", "config_sha256",
                          "ignore_sha256", "schema_version", "artifact_sha256", "artifact_bytes", "metadata_sha256",
                          "expanded_bytes", "project", "export_trigger", "inspection"})
    for key in ("provider_sha256", "config_sha256", "ignore_sha256", "artifact_sha256", "metadata_sha256"):
        digest(manifest[key])
    require(manifest["source_commit"] == "0" * 40 and manifest["provider_version"] == "0.10.8", "manifest source/build")
    require(manifest["manifest_version"] == 1 and manifest["schema_version"] == 1, "manifest schema")
    require(manifest["project"] == "synthetic-artifact-fixture", "public fixture project")
    require(manifest["export_trigger"] == "explicit-index-persistence-true", "intentional fixture export")
    require(manifest["artifact_bytes"] > 0 and manifest["expanded_bytes"] > manifest["artifact_bytes"], "compressed storage")
    inspection = manifest["inspection"]
    exact_keys(inspection, {"sqlite_integrity", "database_identity_confirmed", "table_count", "text_value_count",
                            "absolute_path_values", "source_property_values", "generation"})
    require(inspection["sqlite_integrity"] is True and inspection["database_identity_confirmed"] is True, "SQLite validation")
    require(inspection["generation"] == "synthetic-generation", "inspected synthetic generation")
    for key in ("table_count", "text_value_count", "absolute_path_values", "source_property_values"):
        require(type(inspection[key]) is int and inspection[key] > 0, "actual retained-data inspection")


def verify(report):
    exact_keys(report, {"schema_version", "evidence_kind", "native_performance_evidence", "default_persistence_enabled",
                        "frozen_config_sha256", "provider", "source", "threshold_minimum_improvement_fraction",
                        "repetitions", "samples", "failures", "summary", "comparisons", "complete_cell_accounting",
                        "correctness_passed", "failure_case_accounting", "producer", "decision", "decision_basis",
                        "transport", "remote_download_measured", "limitations", "facts"})
    require(report["schema_version"] == 1 and report["evidence_kind"] == "synthetic", "explicit synthetic evidence")
    require(report["native_performance_evidence"] is False and report["default_persistence_enabled"] is False,
            "no native claim or default persistence")
    require(report["frozen_config_sha256"] is None, "synthetic run is not the frozen experiment")
    require(report["decision"] == "defer" and report["decision_basis"] == "synthetic-cannot-establish-performance",
            "synthetic result cannot authorize adoption")
    exact_keys(report["provider"], {"version", "sha256"})
    require(report["provider"]["version"] == "0.10.8", "audited provider label")
    digest(report["provider"]["sha256"])
    exact_keys(report["source"], {"p0", "p1", "ignore_sha256"})
    require(report["source"]["p0"] == "0" * 40 and report["source"]["p1"] == "1" * 40, "fixture commits")
    exact_keys(report["source"]["ignore_sha256"], {"p0", "p1"})
    for value in report["source"]["ignore_sha256"].values():
        digest(value)
    require(report["repetitions"] == 5 and report["threshold_minimum_improvement_fraction"] == .20, "fixture experiment")
    samples, failures = report["samples"], report["failures"]
    require(len(samples) == 25 and {(s["cell"], s["repetition"]) for s in samples} ==
            {(cell, rep) for cell in CELLS for rep in range(1, 6)}, "all matched cells and repetitions retained")
    require(len(failures) == 4 and {s["cell"] for s in failures} == set(FAILURES), "all four failure cases retained")
    for sample in samples:
        verify_sample(sample)
    for sample in failures:
        require(sample["repetition"] == 1, "one deliberate failure observation per case")
        verify_sample(sample, True)
    exact_keys(report["summary"], CELLS)
    for cell, summary in report["summary"].items():
        exact_keys(summary, {"samples", "ready_correct_count", "clean_initial_count", "consumer_job_median_seconds",
                             "consumer_job_min_seconds", "consumer_job_max_seconds", "ready_correct_median_seconds",
                             "ready_correct_min_seconds", "ready_correct_max_seconds"})
        require(summary["samples"] == 5 and summary["ready_correct_count"] == 5, "complete summary")
        require(summary["clean_initial_count"] == (0 if "artifact" in cell else 5), "fallbacks cannot count as clean imports")
        require(all(value is None for key, value in summary.items() if key.endswith("seconds")), "no synthetic speedup")
    require(len(report["comparisons"]) == 2, "two matched comparisons")
    for comparison, candidate, baseline in zip(report["comparisons"], ("artifact-p0", "stale-artifact-p1"), ("cold-p0", "cold-p1")):
        exact_keys(comparison, {"baseline", "candidate", "improvement_fraction", "threshold_met"})
        require(comparison == {"baseline": baseline, "candidate": candidate, "improvement_fraction": None,
                               "threshold_met": False}, "synthetic comparisons cannot establish threshold")
    for key in ("complete_cell_accounting", "correctness_passed", "failure_case_accounting"):
        require(report[key] is True, "derived report accounting")
    exact_keys(report["producer"], {"manifest", "timing"})
    require(report["producer"]["timing"] == "synthetic-unmeasured", "synthetic producer timing")
    verify_manifest(report["producer"]["manifest"])
    require(report["transport"] == "local-filesystem-copy" and report["remote_download_measured"] is False,
            "no remote transport or Git/server-merge claim")
    require(isinstance(report["limitations"], list) and all(isinstance(s, str) and len(s) < 200 for s in report["limitations"]),
            "bounded limitations")
    facts = report["facts"]
    exact_keys(facts, {"actual_harness_exercised", "failure_fallback_correct", "foreign_root_rejected", "artifact_checksums_verified"})
    require(all(type(value) is bool for value in facts.values()), "closed boolean fixture facts")
    require(all(facts.get(key) is True for key in ("actual_harness_exercised", "failure_fallback_correct",
                                                 "foreign_root_rejected", "artifact_checksums_verified")), "observed fixture facts")
    text = json.dumps(report)
    require(not any(token in text for token in ("/tmp/", "/var/tmp/", "/srv/", "def probe", "def caller", "signature\"")),
            "published report excludes private paths and retained source")
    return ("synthetic-evidence-cannot-authorize-adoption", "all-25-matched-samples-retained",
            "all-four-failure-cases-recovered", "foreign-root-imports-refused-before-recovery",
            "source-symbol-call-coverage-and-deletion-checks-passed", "checksummed-manifest-and-sqlite-inspection",
            "no-synthetic-timing-or-speedup", "closed-report-excludes-source-and-private-paths")


def main():
    verify_harness()
    report = json.loads(Path(sys.argv[1]).read_text())
    checkpoints = verify(report)
    if len(sys.argv) == 3:
        require(report == json.loads(Path(sys.argv[2]).read_text()), "delivered report equals repeated exact harness output")
    for checkpoint in ("exact-repository-harness-bytes", *checkpoints):
        print(FACT_PREFIX + json.dumps({"checkpoint": checkpoint, "passed": True}, sort_keys=True))


if __name__ == "__main__":
    main()
