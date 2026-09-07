"""Closed-fact report projection. Never publish provider text or local paths."""
import math
import statistics

from contract import CELLS, FAILURES

CORRECTNESS_FIELDS = ("current_root_correct", "source_correct", "symbols_correct", "calls_correct",
                      "coverage_confirmed", "negative_checks_correct", "generation", "coverage_signal")
METRIC_FIELDS = ("ready_correct_seconds", "consumer_job_seconds", "checkout_seconds", "transfer_seconds",
                 "transfer_bytes", "index_request_count", "provider_request_count", "initial_index_seconds",
                 "fallback_seconds", "provider_startup_seconds", "local_database_bytes", "retained_cache_bytes",
                 "setup_seconds", "user_cpu_seconds", "system_cpu_seconds", "peak_concurrent_provider_rss_bytes",
                 "rss_sample_count", "sample_interval_seconds", "account_job_seconds", "observed_process_count",
                 "observed_daemon_count", "observed_index_worker_count", "warm_preparation_seconds")
REASONS = {"none", "current-root-mismatch", "source-root-mismatch", "source-range-mismatch", "source-bytes-mismatch",
           "symbol-identity-mismatch", "call-relationship-mismatch", "deleted-symbol-retained", "deleted-graph-file-retained",
           "coverage-generation-unavailable", "coverage-generation-unconfirmed", "coverage-recording-incomplete",
           "coverage-path-unconfirmed", "coverage-path-set-mismatch", "generation-changed-during-query",
           "current-root-changed-during-query", "query-truncated", "query-shape", "query-row-shape", "call-query-truncated",
           "artifact-missing", "artifact-metadata-corrupt", "artifact-manifest-mismatch", "artifact-identity-mismatch",
           "artifact-incompatible-schema", "artifact-source-mismatch", "artifact-size-mismatch", "artifact-checksum-mismatch",
           "artifact-metadata-checksum-mismatch", "provider-request-timeout", "provider-tool-error", "provider-rpc-error",
           "provider-content-not-json", "provider-content-error", "provider-output-closed", "provider-input-closed",
           "provider-response-malformed", "provider-response-too-large", "provider-content-shape", "provider-session-cleanup-forced",
           "index-status-unconfirmed", "index-project-mismatch",
           "local-source-identity-mismatch", "source-path-unavailable", "deleted-local-file-retained", "internal-error"}


def project_sample(sample):
    reason = sample.get("initial_refusal", "none")
    terminal = sample.get("terminal_refusal", "none")
    projected = {"cell": sample["cell"], "repetition": sample["repetition"],
            "source_commit": sample["source_commit"], "daemon_start": sample["daemon_start"],
            "ready_correct": sample.get("ready_correct", False), "recovered": sample.get("recovered", False),
            "initial_refusal": reason if reason in REASONS else "internal-error",
            "terminal_refusal": terminal if terminal in REASONS else "internal-error",
            "natural_account_exit": sample.get("natural_account_exit", False),
            "artifact_accepted": sample.get("artifact_accepted", False),
            "correctness": {key: sample.get("correctness", {}).get(key) for key in CORRECTNESS_FIELDS},
            "metrics": {key: sample.get("metrics", {}).get(key) for key in METRIC_FIELDS}}
    if "provider_failure_probe" in sample:
        probe = sample["provider_failure_probe"]
        projected["provider_failure_probe"] = {key: probe[key] for key in ("attempted", "index_success", "ready_correct", "seconds", "natural_account_exit")}
        projected["provider_failure_probe"]["refusal"] = probe["refusal"] if probe["refusal"] in REASONS else "internal-error"
    if "warm_preparation" in sample:
        projected["warm_preparation"] = {key: sample["warm_preparation"][key] for key in (
            "user_cpu_seconds", "system_cpu_seconds", "peak_concurrent_provider_rss_bytes", "index_request_count", "checkout_seconds")}
    if "index_observations" in sample:
        projected["index_observations"] = [{key: row[key] for key in (
            "status", "project_correct", "artifact_present", "native_import_observed")} for row in sample["index_observations"]]
    return projected


def summarize(samples, native):
    summary = {}
    for cell in CELLS:
        selected = [s for s in samples if s["cell"] == cell]
        # Failed observations are not dropped. No success-only median is emitted.
        complete = native and selected and all(s["ready_correct"] and s["natural_account_exit"] and all(
            isinstance(s["metrics"].get(key), (int, float)) and math.isfinite(s["metrics"][key]) and s["metrics"][key] >= 0
            for key in ("ready_correct_seconds", "consumer_job_seconds")) for s in selected)
        values = [s["metrics"]["consumer_job_seconds"] for s in selected] if complete else []
        ready = [s["metrics"]["ready_correct_seconds"] for s in selected] if complete else []
        summary[cell] = {"samples": len(selected), "ready_correct_count": sum(s["ready_correct"] for s in selected),
                         "clean_initial_count": sum(s["initial_refusal"] == "none" for s in selected),
                         "consumer_job_median_seconds": statistics.median(values) if values else None,
                         "consumer_job_min_seconds": min(values) if values else None,
                         "consumer_job_max_seconds": max(values) if values else None,
                         "ready_correct_median_seconds": statistics.median(ready) if ready else None,
                         "ready_correct_min_seconds": min(ready) if ready else None,
                         "ready_correct_max_seconds": max(ready) if ready else None}
    return summary


def build_report(config, samples, failures, producer, native, frozen_sha, extra=None):
    samples = [project_sample(s) for s in samples]
    failures = [project_sample(s) for s in failures]
    summary = summarize(samples, native)
    expected = config["experiment"]["repetitions"]
    complete = len(samples) == len(CELLS) * expected and all(
        sorted(s["repetition"] for s in samples if s["cell"] == cell) == list(range(1, expected + 1)) for cell in CELLS)
    failure_complete = sorted(s["cell"] for s in failures) == sorted(FAILURES) and all(s["repetition"] == 1 for s in failures)
    correct = complete and failure_complete and all(s["ready_correct"] and s["natural_account_exit"] for s in samples + failures)
    clean = correct and all(s["initial_refusal"] == "none" for s in samples)
    comparisons = []
    threshold = config["experiment"]["minimum_improvement_fraction"]
    for baseline, candidate in (("cold-p0", "artifact-p0"), ("cold-p1", "stale-artifact-p1")):
        a = summary[baseline]["ready_correct_median_seconds"]
        b = summary[candidate]["ready_correct_median_seconds"]
        improvement = (a - b) / a if a and b is not None else None
        baseline_job = summary[baseline]["consumer_job_median_seconds"]
        candidate_job = summary[candidate]["consumer_job_median_seconds"]
        no_job_regression = baseline_job is not None and candidate_job is not None and candidate_job <= baseline_job
        comparisons.append({"baseline": baseline, "candidate": candidate, "improvement_fraction": improvement,
                            "threshold_met": improvement is not None and improvement >= threshold and no_job_regression})
    pilot = native and clean and all(c["threshold_met"] for c in comparisons)
    # Local-copy data never authorizes broad distribution or default persistence.
    return {"schema_version": 1, "evidence_kind": "native" if native else "synthetic",
            "native_performance_evidence": native, "default_persistence_enabled": False,
            "frozen_config_sha256": frozen_sha, "provider": {k: config["provider"][k] for k in ("version", "sha256")},
            "source": {k: config["source"][k] for k in ("p0", "p1", "ignore_sha256")},
            "threshold_minimum_improvement_fraction": threshold, "repetitions": expected,
            "samples": samples, "failures": failures, "summary": summary, "comparisons": comparisons,
            "complete_cell_accounting": complete, "correctness_passed": correct,
            "failure_case_accounting": failure_complete,
            "producer": producer, "decision": "bounded-ci-pilot-candidate" if pilot else "defer",
            "decision_basis": "threshold-and-correctness" if native else "synthetic-cannot-establish-performance",
            "transport": "local-filesystem-copy", "remote_download_measured": False,
            "limitations": ["RSS is a sampled concurrent process sum; brief peaks may be missed.",
                            "Inclusive job child CPU includes native daemon, reaped index workers and control subprocesses; component CPU is not separable.",
                            "Native import, compression and incremental component timers are unavailable; request totals are observed.",
                            "Controlled account root/generation brackets are not atomic snapshot isolation.",
                            "Coverage is best effort; no-recorded-issue is not a proof of exhaustive completeness.",
                            "Local copy is not a remote CI download; Git/server merge behavior is not tested.",
                            "Schema mutation tests rejection, not compatibility with a real next provider build."],
            "facts": extra or {}}
