"""Explicit synthetic adapter for the same artifact/correctness/report pipeline."""
import json
import shutil
import sqlite3
import subprocess
import tempfile
from pathlib import Path

from artifact import make_manifest, quarantine, transfer, validate_artifact
from contract import CELLS, FAILURES, PROVIDER_SHA, Refusal, digest, verify_current
from reporting import build_report


class FixtureClient:
    def __init__(self, root, project, queries):
        self.root, self.project, self.queries = root, project, queries
        self.calls = []

    def call(self, tool, args):
        self.calls.append({"tool": tool})
        symbols = self.queries["symbols"]
        if tool == "index_status":
            return {"root_path": str(self.root.resolve())}
        if tool == "check_index_coverage":
            return {"project": self.project, "indexed_at": "synthetic-generation", "metadata": {
                "generation": "synthetic-generation", "generation_matches": True,
                "hash_records_complete": True, "recording_status": "complete"}, "paths": [
                    {"path": p, "status": "no_recorded_issue", "freshness": "metadata_match"}
                    for p in self.queries["coverage_paths"]]}
        if tool == "search_graph":
            found = [s for s in symbols if args["name_pattern"] == "^" + s["name"] + "$"]
            return {"total": len(found), "has_more": False, "cols": ["name", "label", "lines", "in", "out"],
                    "groups": [{"qn_prefix": self.project, "file": s["file"], "rows": [[
                        s["name"], "Function", f'{s["start_line"]}-{s["end_line"]}', 1, 1]]} for s in found]}
        if tool == "get_code_snippet":
            symbol = next(s for s in symbols if args["qualified_name"] == self.project + "." + s["name"])
            return {"file_path": str(self.root / symbol["file"]), "start_line": symbol["start_line"],
                    "end_line": symbol["end_line"], "source": (self.root / symbol["file"]).read_text()}
        if tool == "trace_path":
            caller = args["function_name"].split(".")[-1]
            return {"callees": {"cols": ["name", "hop"], "groups": [{"qn_prefix": self.project, "rows": [
                [call["callee"], 1] for call in self.queries["calls"] if call["caller"] == caller]}]}}
        raise AssertionError("unexpected fixture request")


def fixture_sources(directory):
    roots, queries = {}, {}
    for revision, value in (("p0", 1), ("p1", 2)):
        root = directory / revision
        root.mkdir()
        items = {"probe": f"def probe():\n    return {value}\n", "caller": "def caller():\n    return probe()\n"}
        if revision == "p0":
            items.update({"deleted": "def deleted():\n    return 3\n", "old_name": "def old_name():\n    return 4\n"})
        else:
            items.update({"added": "def added():\n    return 5\n", "new_name": "def new_name():\n    return 4\n"})
        symbols = []
        for name, content in items.items():
            path = name + ".py"
            (root / path).write_text(content)
            symbols.append({"name": name, "file": path, "start_line": 1, "end_line": 2,
                            "source_sha256": digest(content.encode())})
        queries[revision] = {"symbols": symbols, "calls": [{"caller": "caller", "callee": "probe"}],
                             "coverage_paths": [s["file"] for s in symbols],
                             "absent_symbols": ["deleted", "old_name"] if revision == "p1" else [],
                             "absent_files": ["deleted.py", "old_name.py"] if revision == "p1" else []}
        roots[revision] = root
    return roots, queries


def fixture_artifact(root, directory, config):
    directory.mkdir()
    db_path = directory / "fixture.db"
    db = sqlite3.connect(db_path)
    db.executescript("CREATE TABLE projects(name TEXT,root_path TEXT,indexed_at TEXT); CREATE TABLE nodes(properties TEXT);")
    db.execute("INSERT INTO projects VALUES (?,?,?)", (config["project"], "/synthetic/producer", "synthetic-generation"))
    db.execute("INSERT INTO nodes VALUES (?)", ('{"signature":"synthetic fixture source"}',))
    db.commit()
    db.close()
    archive = directory / "graph.db.zst"
    subprocess.run(["zstd", "-q", "-f", str(db_path), "-o", str(archive)], check=True)
    metadata = {"schema_version": 1, "commit": config["source"]["p0"], "project": config["project"],
                "compressed_size": archive.stat().st_size, "original_size": db_path.stat().st_size}
    (directory / "artifact.json").write_text(json.dumps(metadata))
    db_path.unlink()
    return make_manifest(directory, config, Path("/synthetic/producer"))


def run_fixture(directory):
    roots, queries = fixture_sources(directory)
    config = {"provider": {"version": "0.10.8", "sha256": PROVIDER_SHA}, "project": "synthetic-artifact-fixture",
              "source": {"p0": "0" * 40, "p1": "1" * 40, "ignore_sha256": {"p0": "0" * 64, "p1": "0" * 64}},
              "experiment": {"mode": "full", "provider_environment": {}, "repetitions": 5,
                             "minimum_improvement_fraction": .20, "maximum_consumer_job_regression_fraction": 0.0}}
    artifact = directory / "producer-artifact"
    manifest = fixture_artifact(roots["p0"], artifact, config)
    samples, failures = [], []
    for repetition in range(1, 6):
        for cell in CELLS:
            samples.append(fixture_sample(directory, roots, queries, config, artifact, manifest, cell, repetition))
    for failure in FAILURES:
        failures.append(fixture_sample(directory, roots, queries, config, artifact, manifest, failure, 1))
    report = build_report(config, samples, failures, {"manifest": manifest, "timing": "synthetic-unmeasured"},
                          False, None, {"actual_harness_exercised": True,
                                        "failure_fallback_correct": all(s["initial_refusal"] != "none" and s["recovered"] and s["ready_correct"] for s in failures),
                                        "foreign_root_rejected": all(s["initial_refusal"] == "current-root-mismatch" for s in samples if "artifact" in s["cell"]),
                                        "artifact_checksums_verified": all(s["artifact_accepted"] for s in samples if "artifact" in s["cell"])})
    return report


def fixture_sample(directory, roots, queries, config, artifact, manifest, cell, repetition):
    revision = "p0" if cell.endswith("p0") else "p1"
    consumer = directory / (cell + "-" + str(repetition))
    shutil.copytree(roots[revision], consumer)
    project = config["project"]
    initial_refusal = "none"
    accepted = False
    client = FixtureClient(consumer, project, queries[revision])
    if "artifact" in cell or cell in FAILURES:
        target = consumer / ".codebase-memory"
        transfer(artifact, target, cell if cell in FAILURES else None)
        try:
            validate_artifact(target, config, manifest)
            accepted = True
            # Exercise the installed-build hazard: imported metadata still names producer root.
            verify_current(FixtureClient(roots["p0"], project, queries["p0"]), consumer, project, queries[revision])
        except Refusal as error:
            initial_refusal = str(error)
            quarantine(target)
            client = FixtureClient(consumer, project, queries[revision])
    correctness = verify_current(client, consumer, project, queries[revision])
    return {"cell": cell, "repetition": repetition, "source_commit": config["source"][revision],
            "daemon_start": "synthetic", "initial_refusal": initial_refusal, "artifact_accepted": accepted,
            "ready_correct": True, "recovered": initial_refusal != "none", "natural_account_exit": True,
            "correctness": correctness, "metrics": {"index_request_count": 2 if accepted else 1,
                                                     "provider_request_count": len(client.calls)}}


def fixture_report():
    with tempfile.TemporaryDirectory(prefix="temper-artifact-fixture-") as temporary:
        return run_fixture(Path(temporary))
