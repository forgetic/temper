"""One frozen, alternating paired experiment; all attempts are retained."""

from __future__ import annotations

import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import time
import tomllib

from codex_run import run_codex
from metrics import codex_metrics, comparison, mcp_metrics, temper_metrics
from stack import start
from stack_sessions import read_agent_sessions


def execute(options) -> dict:
    if os.environ.get("TEMPER_BENCHMARK_LIVE") != "1":
        raise ValueError("real model runs require TEMPER_BENCHMARK_LIVE=1")
    root = options.output.resolve()
    if root.is_relative_to(options.repository.resolve()):
        raise ValueError("output must be outside the source repository to avoid inherited task context")
    root.mkdir(parents=True, exist_ok=False, mode=0o700)
    inputs = export_inputs(options.repository.resolve(), options.task_revision, root / "inputs")
    seed = inputs / "fixture/repo"
    task = (inputs / "task.md").read_text()
    configuration = dict(preflight(options))
    frozen_preflight = configuration.copy()
    configuration.update(task_sha256=sha256(inputs / "task.md"),
                         task_revision=git(options.repository, "rev-parse", options.task_revision),
                         input_objects={name: git(options.repository, "rev-parse",
                             f"{options.task_revision}:benchmarks/worker-codex/{name}")
                             for name in ["fixture/repo", "task.md", "acceptance"]},
                         pairs=options.pairs, timeout_seconds=options.timeout_seconds,
                         order=[(["temper", "codex"] if n % 2 else ["codex", "temper"])
                                for n in range(1, options.pairs + 1)])
    write_json(root / "campaign.json", configuration)
    seed_commit(seed)
    configuration["seed_sha"] = git(seed, "rev-parse", "HEAD")
    write_json(root / "campaign.json", configuration)
    trials = []
    for pair, order in enumerate(configuration["order"], 1):
        for ordinal, contestant in enumerate(order, 1):
            arm = root / f"pairs/{pair:03}/{contestant}"
            arm.parent.mkdir(parents=True, exist_ok=True)
            print(f"pair {pair}/{options.pairs}: starting {contestant}", flush=True)
            started = time.monotonic()
            start_load = list(os.getloadavg())
            try:
                if preflight(options) != frozen_preflight:
                    raise ValueError("benchmark configuration or binary changed during the campaign")
                if contestant == "temper":
                    trial = native_arm(seed, task, arm, inputs, options)
                else:
                    trial = codex_arm(seed, task, arm, inputs, options)
            except Exception as error:
                arm.mkdir(parents=True, exist_ok=True)
                trial = {"contestant": contestant, "agent_succeeded": False,
                         "correct": False, "coding_seconds": None,
                         "error": f"{type(error).__name__}: {error}",
                         "tool_calls": None, "graph_calls": None}
            trial.update(pair=pair, order=ordinal, attempt_wall_seconds=time.monotonic() - started,
                         host_load_average_at_start=start_load)
            write_json(arm / "trial.json", trial)
            trials.append(trial)
            write_json(root / "comparison.json", comparison(trials, options.pairs))
            print(f"pair {pair}: {contestant} correct={trial['correct']} "
                  f"coding_seconds={trial.get('coding_seconds')}", flush=True)
    result = comparison(trials, options.pairs)
    write_json(root / "comparison.json", result)
    return result


def native_arm(seed, task, arm, inputs, options):
    proxy = Path(__file__).with_name("mcp_proxy.py").resolve()
    mcp = {"mode": "required", "command": sys.executable,
           "args": [str(proxy), "--log", str(arm / "mcp.jsonl"), "--", str(options.mcp_bin)],
           "roles": ["engineer"], "index": "background", "startup_timeout_secs": 30,
           "index_timeout_secs": 60, "retention": {"enabled": False}}
    delivery = None
    delivery_error = None
    with start(seed=seed, root=arm, temper_bin=options.temper_bin,
               fixture_bin=options.fixture_bin, auth_file=options.auth_file,
               codebase_memory=mcp) as stack:
        if stack.seed_sha != git(seed, "rev-parse", "HEAD"):
            raise ValueError("native seed commit differs from the frozen common seed")
        try:
            delivery = stack.run(task, timeout_seconds=options.timeout_seconds)
        except Exception as error:
            delivery_error = f"{type(error).__name__}: {error}"
        journal_root = stack.journal_root
    sessions = read_agent_sessions(arm / "temper.log", require_complete=False)
    summaries, analysis = analyze_native(journal_root, arm, options.analyzer_bin)
    trial = temper_metrics(summaries, sessions)
    analysis["invocations"] = len(sessions)
    if analysis["traces"] != len(sessions):
        analysis["errors"].append({"error": "native trace and invocation counts differ"})
    analysis["complete"] = (bool(sessions) and not analysis["errors"]
                            and len(summaries) == len(sessions))
    if not analysis["complete"]:
        # These are known subtotals, not complete invocation-level aggregates.
        fields = ["tool_calls", "tools_by_name", "tool_failures", "graph_calls", "graph_by_name",
                  "model_calls", "model_attempts", "model_seconds", "tokens"]
        analysis["observed_metrics"] = {key: trial[key] for key in fields}
        trial.update({key: None for key in fields})
        trial.update(agent_succeeded=False, tool_evidence_complete=False)
    trial.update(delivery=delivery, error=delivery_error, agent_sessions=sessions, analysis=analysis)
    trial["mcp"] = mcp_metrics(arm / "mcp.jsonl")
    if delivery is None:
        trial["correct"] = False
    else:
        validate_trial(trial, Path(delivery["final_checkout"]), seed, inputs, arm)
    return trial


def analyze_native(journal_root, arm, analyzer):
    summaries, errors = [], []
    manifests = sorted(Path(journal_root).rglob("manifest.json"))
    for index, manifest in enumerate(manifests):
        analysis = arm / "analysis" / f"{index:03}"
        log_path = arm / f"analysis-{index}.log"
        try:
            with log_path.open("wb") as log:
                subprocess.run([str(analyzer), "analyze", "--trace", str(manifest.parent),
                                "--output-dir", str(analysis)], stdout=log,
                               stderr=subprocess.STDOUT, check=True)
            summary = json.loads((analysis / "run.json").read_text())
            temper_metrics([summary], [])  # Reject malformed summaries before aggregation.
            summaries.append(summary)
        except Exception as error:
            detail = f"{type(error).__name__}: {error}"
            with log_path.open("a") as log:
                log.write("\n" + detail + "\n")
            errors.append({"trace": str(manifest.parent), "log": str(log_path), "error": detail})
    return summaries, {"traces": len(manifests), "analyzed_traces": len(summaries), "errors": errors}


def codex_arm(seed, task, arm, inputs, options):
    arm.mkdir()
    checkout = arm / "repo"
    subprocess.run(["git", "clone", "--quiet", "--no-hardlinks", str(seed), str(checkout)], check=True)
    process = run_codex(checkout, task, arm / "session", executable=str(options.codex_bin),
                        timeout_seconds=options.timeout_seconds,
                        mcp_proxy=Path(__file__).with_name("mcp_proxy.py"), mcp_binary=options.mcp_bin)
    trial = codex_metrics(arm / "session/events.jsonl", process)
    trial["mcp"] = mcp_metrics(arm / "session/mcp.jsonl")
    validate_trial(trial, checkout, seed, inputs, arm)
    return trial


def validate_trial(trial, checkout, seed, inputs, output):
    started = time.monotonic()
    evidence = {"passed": False, "complete": False}
    try:
        evidence.update(validate(checkout, seed, inputs, output, evidence=evidence))
        evidence["complete"] = True
    except Exception as error:
        evidence.update(passed=False, complete=False, error=f"{type(error).__name__}: {error}")
        (output / "validation-error.log").write_text(evidence["error"] + "\n")
    evidence["duration_seconds"] = time.monotonic() - started
    trial["validation"] = evidence
    trial["correct"] = trial["agent_succeeded"] and evidence["passed"]
    write_json(output / "validation.json", evidence)


def validate(checkout, seed, inputs, output, *, evidence=None):
    evidence = {} if evidence is None else evidence
    gates = evidence.setdefault("gates", [])
    for name, command in [("format", ["cargo", "fmt", "--all", "--", "--check"]),
                          ("tests", ["cargo", "test", "--offline", "--quiet"])]:
        started = time.monotonic()
        gate = {"name": name, "passed": False, "complete": False}
        try:
            with (output / f"host-{name}.log").open("wb") as log:
                result = subprocess.run(command, cwd=checkout, stdout=log, stderr=subprocess.STDOUT,
                                        timeout=120)
            gate.update(passed=result.returncode == 0, complete=True)
        finally:
            gate["duration_seconds"] = time.monotonic() - started
            gates.append(gate)
    oracle_path = output / "acceptance.json"
    started = time.monotonic()
    try:
        with (output / "host-oracle.log").open("wb") as log:
            subprocess.run([sys.executable, str(inputs / "acceptance/run.py"), str(checkout),
                            "--json-output", str(oracle_path)], stdout=log,
                           stderr=subprocess.STDOUT, timeout=150)
        oracle = json.loads(oracle_path.read_text())
        evidence.update(oracle_passed=oracle["passed"], oracle_seconds=oracle["duration_seconds"])
    finally:
        evidence["oracle_wall_seconds"] = time.monotonic() - started
    manifest = tomllib.loads((checkout / "Cargo.toml").read_text())
    dependency_free = not any(manifest.get(key) for key in
                              ["dependencies", "dev-dependencies", "build-dependencies", "target"])
    evidence["dependency_free"] = dependency_free
    unchanged_gates = all((checkout / name).read_bytes() == (seed / name).read_bytes() for name in
                          [".temper/pre-push.toml", ".forgejo/workflows/ci.yml", "AGENTS.md"])
    evidence["unchanged_gates"] = unchanged_gates
    docs_changed = all((checkout / name).read_bytes() != (seed / name).read_bytes()
                       for name in ["README.md", "CHANGELOG.md"])
    evidence["docs_changed"] = docs_changed
    capture_patch(checkout, git(seed, "rev-parse", "HEAD"), output)
    untracked = git(checkout, "ls-files", "--others", "--exclude-standard")
    (output / "untracked-files.txt").write_text(untracked + "\n")
    evidence.update({"passed": all(g["passed"] for g in gates) and oracle["passed"] and dependency_free
            and unchanged_gates and docs_changed, "gates": gates, "oracle_passed": oracle["passed"],
            "oracle_seconds": oracle["duration_seconds"], "dependency_free": dependency_free,
            "unchanged_gates": unchanged_gates, "docs_changed": docs_changed})
    return evidence


def capture_patch(checkout, baseline, output):
    index = (output / "snapshot.index").resolve()
    environment = os.environ.copy() | {"GIT_INDEX_FILE": str(index)}
    try:
        for args in [["read-tree", "HEAD"], ["add", "--all"]]:
            subprocess.run(["git", *args], cwd=checkout, env=environment, check=True)
        patch = subprocess.check_output(["git", "diff", "--cached", "--binary", baseline],
                                        cwd=checkout, env=environment)
        (output / "candidate.patch").write_bytes(patch)
    finally:
        index.unlink(missing_ok=True)


def preflight(options):
    codex_home = Path(os.environ.get("CODEX_HOME", Path.home() / ".codex"))
    config_path = codex_home / "config.toml"
    config = tomllib.loads(config_path.read_text())
    mcp = config.get("mcp_servers", {}).get("codebase-memory-mcp", {})
    if not mcp or mcp.get("enabled") is False:
        raise ValueError("Codex default configuration must enable codebase-memory-mcp")
    if config.get("service_tier") not in (None, "default", "auto"):
        raise ValueError("Codex service tier must match Temper's provider default")
    if config.get("model_provider", "openai") != "openai":
        raise ValueError("Codex must use the same OpenAI provider as Temper")
    codex_auth = json.loads((codex_home / "auth.json").read_text())
    native_auth = json.loads(options.auth_file.read_text())
    codex_account = codex_auth.get("tokens", {}).get("account_id")
    native_account = native_auth.get("openai-codex", {}).get("accountId")
    if not codex_account or codex_account != native_account:
        raise ValueError("Codex and Temper OAuth files must identify the same OpenAI account")
    for path in [options.temper_bin, options.fixture_bin, options.analyzer_bin, options.codex_bin,
                 options.mcp_bin, options.auth_file]:
        if not path.is_file():
            raise ValueError(f"required file missing: {path}")
    return {"schema_version": 1, "model": "gpt-6-astra", "reasoning_effort": "xhigh",
            "service_tier": "provider_default", "same_openai_account": True,
            "provider_reported_model": None, "provider_reported_reasoning_effort": None,
            "host_cpu_count": os.cpu_count(),
            "harness_sources": {path.name: sha256(path)
                                for path in sorted(Path(__file__).parent.glob("*.py"))},
            "codex_config_sha256": sha256(config_path),
            "codex_instructions_sha256": (sha256(codex_home / "AGENTS.md")
                                          if (codex_home / "AGENTS.md").exists() else None),
            "binaries": {name: sha256(getattr(options, name)) for name in
                         ["temper_bin", "fixture_bin", "analyzer_bin", "codex_bin", "mcp_bin"]},
            "versions": {"codex": subprocess.check_output([str(options.codex_bin), "--version"], text=True).strip(),
                         "mcp": subprocess.check_output([str(options.mcp_bin), "--version"], text=True).strip(),
                         "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip()},
            "cache_policy": "warm toolchain/shared MCP daemon; cold per-arm checkout, target, and project"}


def export_inputs(repository, revision, output):
    prefix = "benchmarks/worker-codex"
    payload = subprocess.check_output(["git", "-C", str(repository), "archive", revision, prefix])
    output.mkdir(parents=True)
    with tarfile.open(fileobj=io.BytesIO(payload)) as archive:
        archive.extractall(output, filter="data")
    return output / prefix


def seed_commit(seed):
    environment = os.environ.copy() | {"GIT_AUTHOR_DATE": "2000-01-01T00:00:00Z",
                                      "GIT_COMMITTER_DATE": "2000-01-01T00:00:00Z"}
    for args in [["init", "-q", "-b", "main"], ["config", "user.name", "Benchmark Admin"],
                 ["config", "user.email", "benchmark-admin@example.invalid"], ["add", "--all"],
                 ["commit", "-q", "-m", "Frozen benchmark baseline"]]:
        subprocess.run(["git", *args], cwd=seed, env=environment, check=True)


def git(root, *args):
    return subprocess.check_output(["git", "-C", str(root), *args], text=True).strip()


def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")
