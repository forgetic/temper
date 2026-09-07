"""Frozen experiment inputs and bounded, public correctness checks."""
import hashlib
import json
import re
from pathlib import Path

CELLS = ("cold-p0", "artifact-p0", "cold-p1", "stale-artifact-p1", "warm-incremental-p1")
FAILURES = ("missing", "corrupt", "truncated", "incompatible")
PROVIDER_SHA = "cb6dc545a8cb714799461ba7d8c223dbca7d02ca46ea5a4215c1beb450fd8e09"


class Refusal(Exception):
    """Closed reason code; never interpolate private provider output into it."""


def digest(data):
    return hashlib.sha256(data).hexdigest()


def identity(value):
    return digest(json.dumps(value, sort_keys=True, separators=(",", ":")).encode())


def file_digest(path):
    h = hashlib.sha256()
    with open(path, "rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def relative_path(value):
    p = Path(value)
    if not value or p.is_absolute() or ".." in p.parts or str(p) != value:
        raise Refusal("unsafe-relative-path")
    return p


def validate_config(config):
    if config.get("schema_version") != 1:
        raise Refusal("config-version")
    provider = config["provider"]
    if provider["sha256"] != PROVIDER_SHA or provider["version"] != "0.10.8":
        raise Refusal("unsupported-provider-build")
    for revision in ("p0", "p1"):
        if not re.fullmatch("[0-9a-f]{40}", config["source"][revision]):
            raise Refusal("unpinned-source")
        if not re.fullmatch("[0-9a-f]{64}", config["source"]["ignore_sha256"][revision]):
            raise Refusal("unpinned-ignore-policy")
        queries = config["queries"][revision]
        if not queries["symbols"] or not queries["calls"] or not queries["coverage_paths"]:
            raise Refusal("incomplete-query-set")
        names = set()
        for symbol in queries["symbols"]:
            relative_path(symbol["file"])
            if symbol["name"] in names or not re.fullmatch("[A-Za-z_][A-Za-z_0-9]*", symbol["name"]):
                raise Refusal("ambiguous-query-symbol")
            names.add(symbol["name"])
            if not 1 <= symbol["start_line"] <= symbol["end_line"]:
                raise Refusal("invalid-source-range")
            if not re.fullmatch("[0-9a-f]{64}", symbol["source_sha256"]):
                raise Refusal("unpinned-source-range")
        for call in queries["calls"]:
            if call["caller"] not in names or call["callee"] not in names:
                raise Refusal("unbound-call-query")
        for path in queries["coverage_paths"] + queries.get("absent_files", []):
            relative_path(path)
        if set(queries["coverage_paths"]) != {s["file"] for s in queries["symbols"]}:
            raise Refusal("uncovered-query-source")
    if not config["queries"]["p1"].get("absent_symbols") or not config["queries"]["p1"].get("absent_files"):
        raise Refusal("missing-negative-query-set")
    run = config["experiment"]
    if run["repetitions"] < 5 or len(run["order"]) != run["repetitions"]:
        raise Refusal("insufficient-repetitions")
    if any(sorted(row) != sorted(CELLS) for row in run["order"]):
        raise Refusal("unmatched-cell-order")
    if not 0 < run["minimum_improvement_fraction"] < 1:
        raise Refusal("invalid-threshold")
    if run["maximum_consumer_job_regression_fraction"] != 0:
        raise Refusal("consumer-job-regression-not-allowed")
    if not 0 < run["sample_interval_seconds"] <= 1:
        raise Refusal("invalid-resource-sampling")
    if run["timeout_seconds"] <= 0 or run["shutdown_timeout_seconds"] <= 0:
        raise Refusal("invalid-deadline")
    if run["uid"] <= 0 or run["gid"] <= 0 or run["mode"] not in ("fast", "moderate", "full"):
        raise Refusal("invalid-isolation-or-mode")
    allowed_env = {"CBM_INDEX_SINGLE_THREAD", "CBM_PROFILE"}
    if set(run["provider_environment"]) - allowed_env:
        raise Refusal("unapproved-provider-environment")
    if not re.fullmatch("[a-z][a-z0-9-]{0,63}", config["project"]):
        raise Refusal("unsafe-project-name")
    return config


def grouped_rows(result):
    """Decode the audited provider's JSON tree; reject truncated pages."""
    if result.get("has_more") or result.get("truncated") or result.get("next") or result.get("next_cursor"):
        raise Refusal("query-truncated")
    if not isinstance(result.get("groups"), list) or not isinstance(result.get("cols"), list):
        raise Refusal("query-shape")
    rows = []
    for group in result["groups"]:
        for values in group["rows"]:
            if len(values) != len(result["cols"]):
                raise Refusal("query-row-shape")
            row = dict(zip(result["cols"], values))
            row["qualified_name"] = ".".join(filter(None, (group["qn_prefix"], row["name"])))
            row["file"] = group.get("file")
            rows.append(row)
    return rows


def local_source(root, expected, snippet):
    target = (root / relative_path(expected["file"])).resolve()
    if not target.is_relative_to(root.resolve()) or not target.is_file():
        raise Refusal("source-path-unavailable")
    returned = Path(snippet.get("file_path", ""))
    if not returned.is_absolute():
        returned = root / returned
    if returned.resolve() != target:
        raise Refusal("source-root-mismatch")
    if (snippet.get("start_line"), snippet.get("end_line")) != (expected["start_line"], expected["end_line"]):
        raise Refusal("source-range-mismatch")
    lines = target.read_bytes().splitlines(keepends=True)
    exact = b"".join(lines[expected["start_line"] - 1:expected["end_line"]])
    if digest(exact) != expected["source_sha256"]:
        raise Refusal("local-source-identity-mismatch")
    if not isinstance(snippet.get("source"), str) or snippet["source"].encode() != exact:
        raise Refusal("source-bytes-mismatch")


def coverage_gate(result, project, paths):
    meta = result.get("metadata", {})
    generation = meta.get("generation")
    if result.get("project") != project or not generation or generation != result.get("indexed_at"):
        raise Refusal("coverage-generation-unavailable")
    if meta.get("generation_matches") is not True or meta.get("hash_records_complete") is not True:
        raise Refusal("coverage-generation-unconfirmed")
    if meta.get("recording_status") != "complete":
        raise Refusal("coverage-recording-incomplete")
    entries = result.get("paths", [])
    if sorted(row.get("path", "") for row in entries) != sorted(paths):
        raise Refusal("coverage-path-set-mismatch")
    for row in entries:
        if row.get("status") != "no_recorded_issue" or row.get("freshness") != "metadata_match":
            raise Refusal("coverage-path-unconfirmed")
    return generation


def verify_current(client, root, project, queries):
    """Ready means root, generation, source, symbols, calls and coverage agree."""
    before = client.call("index_status", {"project": project})
    if before.get("root_path") != str(root.resolve()):
        raise Refusal("current-root-mismatch")
    covered = client.call("check_index_coverage", {"project": project, "paths": queries["coverage_paths"]})
    generation = coverage_gate(covered, project, queries["coverage_paths"])
    qualified = {}
    for expected in queries["symbols"]:
        result = client.call("search_graph", {"project": project, "name_pattern": "^" + expected["name"] + "$",
                                              "format": "json", "limit": 100})
        matches = [r for r in grouped_rows(result) if r["file"] == expected["file"]]
        if len(matches) != 1 or matches[0]["lines"] != f'{expected["start_line"]}-{expected["end_line"]}':
            raise Refusal("symbol-identity-mismatch")
        qualified[expected["name"]] = matches[0]["qualified_name"]
        snippet = client.call("get_code_snippet", {"project": project, "qualified_name": matches[0]["qualified_name"]})
        local_source(root, expected, snippet)
    for expected in queries["calls"]:
        result = client.call("trace_path", {"project": project, "function_name": qualified[expected["caller"]],
                                            "direction": "outbound", "depth": 1, "include_tests": True,
                                            "format": "json", "limit": 100})
        if result.get("truncated") or result.get("next"):
            raise Refusal("call-query-truncated")
        callees = grouped_rows(result.get("callees", {}))
        if not any(r["qualified_name"] == qualified[expected["callee"]] and r["hop"] == 1 for r in callees):
            raise Refusal("call-relationship-mismatch")
    for name in queries.get("absent_symbols", []):
        result = client.call("search_graph", {"project": project, "name_pattern": "^" + re.escape(name) + "$",
                                              "format": "json", "limit": 100})
        if result.get("total") != 0 or grouped_rows(result):
            raise Refusal("deleted-symbol-retained")
    for path in queries.get("absent_files", []):
        if (root / relative_path(path)).exists():
            raise Refusal("deleted-local-file-retained")
        result = client.call("search_graph", {"project": project, "name_pattern": ".*", "file_pattern": path,
                                              "format": "json", "limit": 100})
        if result.get("total") != 0 or grouped_rows(result):
            raise Refusal("deleted-graph-file-retained")
    final_coverage = client.call("check_index_coverage", {"project": project, "paths": queries["coverage_paths"]})
    if coverage_gate(final_coverage, project, queries["coverage_paths"]) != generation:
        raise Refusal("generation-changed-during-query")
    after = client.call("index_status", {"project": project})
    if after.get("root_path") != str(root.resolve()):
        raise Refusal("current-root-changed-during-query")
    return {"current_root_correct": True, "source_correct": True, "symbols_correct": True,
            "calls_correct": True, "coverage_confirmed": True, "generation": generation,
            "negative_checks_correct": True, "coverage_signal": "best_effort"}
