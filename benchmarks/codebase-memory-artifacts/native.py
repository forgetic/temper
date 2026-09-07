"""Opt-in experiment orchestration. Only public provider commands and MCP."""
import json
import os
import platform
import shutil
import subprocess
import time
from pathlib import Path

from accounting import AccountMeter, processes, require_unused, subreaper
from artifact import make_manifest, quarantine, transfer, validate_artifact
from contract import FAILURES, Refusal, file_digest, verify_current
from protocol import McpClient
from reporting import build_report
from source import checkout, confirm_checkout, git, own_tree, validate_sources, write_json


class NativeExperiment:
    def __init__(self, config, state, frozen_sha):
        self.config, self.state, self.frozen_sha = config, state, frozen_sha
        self.run = config["experiment"]
        self.uid, self.gid = self.run["uid"], self.run["gid"]
        subreaper()
        require_unused(self.uid)
        if file_digest(config["provider"]["path"]) != config["provider"]["sha256"]:
            raise Refusal("provider-executable-checksum-mismatch")
        validate_sources(config)
        state.mkdir(mode=0o700)  # Existing state is never adopted or overwritten.
        os.chown(state, self.uid, self.gid)
        self.cache, self.runtime = state / "cache", state / "runtime"
        self.raw = state / "private-evidence"
        for path in (self.cache, self.runtime, self.raw, state / "config", state / "xdg-cache"):
            path.mkdir(mode=0o700)
            os.chown(path, self.uid, self.gid)
        stat = state.stat()
        write_json(state / "ownership.json", {"kind": "temper-artifact-benchmark-v1", "root": str(state.resolve()),
                   "device": stat.st_dev, "inode": stat.st_ino, "uid": self.uid, "config_sha256": frozen_sha})
        self.environment = {"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8", "CBM_CACHE_DIR": str(self.cache),
                            "CBM_RUNTIME_DIR": str(self.runtime), "XDG_CONFIG_HOME": str(state / "config"),
                            "XDG_CACHE_HOME": str(state / "xdg-cache"), "XDG_RUNTIME_DIR": str(self.runtime),
                            "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": "/dev/null",
                            "GIT_CONFIG_SYSTEM": "/dev/null", "GIT_TERMINAL_PROMPT": "0"}
        # Config CLI requires HOME to exist, although CBM_CACHE_DIR determines its store.
        # Preserve the inherited value; every cache/config/runtime location is overridden.
        if "HOME" in os.environ:
            self.environment["HOME"] = os.environ["HOME"]
        self.environment.update(self.run["provider_environment"])
        self.session_number = 0
        self.setup_seconds = self.configure()

    def configure(self):
        started = time.monotonic()
        for name in ("auto_index", "auto_watch", "ui_enabled"):
            for args in (("set", name, "false"), ("get", name)):
                result = subprocess.run([self.config["provider"]["path"], "config", *args], cwd=self.state,
                                        env=self.environment, user=self.uid, group=self.gid, extra_groups=[],
                                        stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30, check=True)
                if args[0] == "get" and result.stdout.strip() != b"false":
                    raise Refusal("background-suppression-unconfirmed")
        if processes(self.uid):
            raise Refusal("config-left-account-processes")
        return time.monotonic() - started

    def archive_cache(self, label):
        if processes(self.uid):
            raise Refusal("cache-rotation-while-account-active")
        destination = self.raw / (label + "-cache")
        destination.mkdir()
        # Preserve the configured store, all three settings, and canonical path.
        for path in self.cache.iterdir():
            if path.name in ("_config.db", "_config.db-wal", "_config.db-shm", "config.json"):
                continue
            shutil.move(str(path), destination / path.name)
        return sum(p.stat().st_size for p in destination.rglob("*") if p.is_file())

    def prepare_checkout(self, name, revision):
        root = self.state / name
        started = time.monotonic()
        checkout(self.config["source"]["repository"], self.config["source"][revision], root, self.uid, self.gid)
        confirm_checkout(root, self.config["source"][revision])
        return root, time.monotonic() - started

    def session(self, root, project, label):
        self.session_number += 1
        return NativeSession(self, root, project, label + "-" + str(self.session_number))

    def produce(self):
        root, checkout_seconds = self.prepare_checkout("producer", "p0")
        session = self.session(root, self.config["project"], "producer")
        result = {"checkout_seconds": checkout_seconds, "setup_seconds": self.setup_seconds}
        try:
            started = time.monotonic()
            session.index(persistence=True)
            result["correctness"] = session.verify(self.config["queries"]["p0"])
            result["index_export_correct_seconds"] = time.monotonic() - started
        finally:
            result.update(session.close())
        if not result["natural_account_exit"]:
            raise Refusal("producer-account-lifetime-failed")
        artifact = root / ".codebase-memory"
        started = time.monotonic()
        result["manifest"] = make_manifest(artifact, self.config, root)
        result["artifact_inspection_seconds"] = time.monotonic() - started
        result["retained_cache_bytes"] = self.archive_cache("producer")
        write_json(self.raw / "producer.json", result)
        return artifact, result

    def sample(self, cell, repetition, artifact, manifest):
        revision = "p0" if cell.endswith("p0") else "p1"
        name = f"{cell}-{repetition}"
        job_started = time.monotonic()
        root, checkout_seconds = self.prepare_checkout(name, "p0" if cell == "warm-incremental-p1" else revision)
        sample = {"cell": cell, "repetition": repetition, "source_commit": self.config["source"][revision],
                  "daemon_start": "warm" if cell == "warm-incremental-p1" else "cold", "initial_refusal": "none",
                  "terminal_refusal": "none", "ready_correct": False, "recovered": False,
                  "artifact_accepted": False, "metrics": {"checkout_seconds": checkout_seconds, "fallback_seconds": 0.0}}
        session = None
        measurements = []
        ready_started = time.monotonic()
        try:
            if cell == "warm-incremental-p1":
                session = self.session(root, self.config["project"], name + "-warm")
                session.index()
                session.verify(self.config["queries"]["p0"])
                sample["metrics"]["warm_preparation_seconds"] = time.monotonic() - job_started
                # Warm preparation is explicitly outside the consumer metric.
                job_started = ready_started = time.monotonic()
                sample["warm_preparation"] = session.reset_phase()
                sample["warm_preparation"]["checkout_seconds"] = checkout_seconds
                checkout_started = time.monotonic()
                git(root, "checkout", "--quiet", "--detach", self.config["source"]["p1"])
                own_tree(root, self.uid, self.gid)
                sample["metrics"]["checkout_seconds"] = time.monotonic() - checkout_started
                ready_started = time.monotonic()
            if "artifact" in cell or cell in FAILURES:
                target = root / ".codebase-memory"
                sample["metrics"].update(transfer(artifact, target, cell if cell in FAILURES else None))
                own_tree(target, self.uid, self.gid)
                try:
                    validate_artifact(target, self.config, manifest)
                    sample["artifact_accepted"] = True
                except Refusal as error:
                    sample["initial_refusal"] = str(error)
                    if cell in FAILURES:
                        sample["provider_failure_probe"], probe = self.failure_probe(root, name, revision)
                        measurements.append(probe)
                    quarantine(target)
            if session is None:
                fallback_started = time.monotonic()
                project = self.config["project"] + ("-recovery" if sample["initial_refusal"] != "none" else "")
                session = self.session(root, project, name)
            started = time.monotonic()
            session.index()
            sample["metrics"]["initial_index_seconds"] = time.monotonic() - started
            sample["correctness"] = session.verify(self.config["queries"][revision])
            sample["ready_correct"] = True
            # A pre-import wrapper refusal followed by indexing is explicit recovery.
            sample["recovered"] = sample["initial_refusal"] != "none"
            if sample["recovered"]:
                sample["metrics"]["fallback_seconds"] = time.monotonic() - fallback_started
        except Refusal as error:
            if hasattr(error, "session_measurements"):
                measurements.append(error.session_measurements)
            if sample["initial_refusal"] == "none":
                sample["initial_refusal"] = str(error)
            if session:
                measurements.append(session.close())
                session = None
            quarantine(root / ".codebase-memory")
            recovery_started = time.monotonic()
            try:
                # A new named project forces absent local DB without cache deletion.
                session = self.session(root, self.config["project"] + "-recovery", name + "-recovery")
                session.index()
                sample["correctness"] = session.verify(self.config["queries"][revision])
                sample["ready_correct"] = True
                sample["recovered"] = True
            except Refusal as terminal:
                if hasattr(terminal, "session_measurements"):
                    measurements.append(terminal.session_measurements)
                sample["terminal_refusal"] = str(terminal)
            sample["metrics"]["fallback_seconds"] = time.monotonic() - recovery_started
        finally:
            sample["metrics"]["ready_correct_seconds"] = time.monotonic() - ready_started if sample["ready_correct"] else None
            if session:
                measurements.append(session.close())
            sample["natural_account_exit"] = bool(measurements) and all(m["natural_account_exit"] for m in measurements)
            sample["metrics"].update(merge_measurements(measurements))
            sample["metrics"]["local_database_bytes"] = sum(p.stat().st_size for p in self.cache.glob("*.db") if p.name != "_config.db")
            sample["metrics"]["consumer_job_seconds"] = time.monotonic() - job_started
            sample["index_observations"] = [row for m in measurements for row in m["index_observations"]]
            sample["metrics"]["retained_cache_bytes"] = self.archive_cache(name)
            confirm_checkout(root, self.config["source"][revision])
            write_json(self.raw / (name + ".json"), sample)
        return sample

    def failure_probe(self, root, name, revision):
        session = None
        result = {"attempted": True, "index_success": False, "ready_correct": False, "refusal": "none"}
        started = time.monotonic()
        try:
            session = self.session(root, self.config["project"], name + "-raw-failure-probe")
            session.index()
            result["index_success"] = True
            session.verify(self.config["queries"][revision])
            result["ready_correct"] = True
        except Refusal as error:
            result["refusal"] = str(error)
            if hasattr(error, "session_measurements"):
                measured = error.session_measurements
        finally:
            if session:
                measured = session.close()
        result["seconds"] = time.monotonic() - started
        result["natural_account_exit"] = measured["natural_account_exit"]
        self.archive_cache(name + "-raw-probe")
        return result, measured


class NativeSession:
    def __init__(self, experiment, root, project, label):
        self.experiment, self.root, self.project, self.label = experiment, root, project, label
        self.clients = []
        self.prior_forced_cleanup = False
        self.index_observations = []
        self.closed_result = None
        self.meter = AccountMeter(experiment.uid, experiment.run["sample_interval_seconds"], experiment.cache)
        try:
            self.bootstrap = self.client("analysis")
        except BaseException as error:
            error.session_measurements = self.close()
            raise

    def client(self, profile):
        e = self.experiment
        client = McpClient(e.config["provider"]["path"], self.root, e.environment, e.uid, e.gid, profile,
                           e.run["timeout_seconds"], e.raw / (self.label + ".jsonl"),
                           self.label + "-client-" + str(len(self.clients)))
        self.clients.append(client)
        client.initialize()
        return client

    def index(self, persistence=False):
        writer = self.client("all")
        try:
            result = writer.call("index_repository", {"repo_path": str(self.root), "name": self.project,
                                 "mode": self.experiment.run["mode"], "persistence": persistence})
            status = result.get("status")
            self.index_observations.append({"status": status if status in ("indexed", "degraded", "error") else "unknown",
                                            "project_correct": result.get("project") == self.project,
                                            "artifact_present": result.get("artifact_present") is True,
                                            "native_import_observed": False})
            if result.get("project") != self.project:
                raise Refusal("index-project-mismatch")
            if status != "indexed":
                raise Refusal("index-status-unconfirmed")
            return result
        finally:
            writer.close()

    def verify(self, queries):
        # Native sessions retain opened generations, so each verification gets a fresh reader.
        reader = self.client("analysis")
        try:
            return verify_current(reader, self.root, self.project, queries)
        finally:
            reader.close()

    def reset_phase(self):
        previous = self.meter.reset_phase()
        previous["index_request_count"] = sum(c["tool"] == "index_repository" for client in self.clients for c in client.calls)
        self.index_observations = []
        self.prior_forced_cleanup |= any(client.forced_cleanup for client in self.clients)
        self.clients = [self.bootstrap]
        self.bootstrap.calls.clear()
        self.bootstrap.startup_seconds = 0.0
        return previous

    def close(self):
        if self.closed_result is not None:
            return self.closed_result
        for client in reversed(self.clients):
            client.close()
        result = self.meter.finish(self.experiment.run["shutdown_timeout_seconds"])
        if self.prior_forced_cleanup or any(client.forced_cleanup for client in self.clients) or not result["cpu_accounting_consistent"]:
            result["natural_account_exit"] = False
        calls = [call for client in self.clients for call in client.calls]
        result.update({"index_request_count": sum(c["tool"] == "index_repository" for c in calls),
                       "provider_request_count": len(calls),
                       "provider_startup_seconds": sum(c.startup_seconds for c in self.clients)})
        result["index_observations"] = self.index_observations
        write_json(self.experiment.raw / (self.label + "-processes.json"), {
            "processes": [{"pid": pid, "start_tick": start, **data} for (pid, start), data in self.meter.seen.items()],
            "worker_start_headers": self.meter.worker_logs})
        self.closed_result = result
        return result


def merge_measurements(measurements):
    keys = ("user_cpu_seconds", "system_cpu_seconds", "account_job_seconds", "observed_process_count",
            "observed_daemon_count", "observed_index_worker_count", "provider_request_count", "index_request_count",
            "provider_startup_seconds", "rss_sample_count")
    result = {key: sum(m[key] for m in measurements) for key in keys}
    result["peak_concurrent_provider_rss_bytes"] = max((m["peak_concurrent_provider_rss_bytes"] for m in measurements), default=0)
    result["sample_interval_seconds"] = measurements[0]["sample_interval_seconds"] if measurements else None
    return result


def run_native(config, state, frozen_sha, progress, frozen_bytes):
    started = time.monotonic()
    experiment = NativeExperiment(config, state, frozen_sha)
    (experiment.raw / "frozen-config.json").write_bytes(frozen_bytes)
    write_json(experiment.raw / "runtime-config.json", config)
    write_json(experiment.raw / "host.json", {"platform": platform.platform(), "cpu_count": os.cpu_count(),
               "load_average": os.getloadavg(), "cpuinfo": Path("/proc/cpuinfo").read_text(),
               "meminfo": Path("/proc/meminfo").read_text(), "mountinfo": Path("/proc/self/mountinfo").read_text()})
    progress("producer-start")
    artifact, producer = experiment.produce()
    samples, failures = [], []
    for repetition, order in enumerate(config["experiment"]["order"], 1):
        for cell in order:
            progress(f"sample-start {cell} {repetition}")
            sample = experiment.sample(cell, repetition, artifact, producer["manifest"])
            samples.append(sample)
            progress(f"sample-end {cell} {repetition} ready={sample['ready_correct']} initial={sample['initial_refusal']}")
    for failure in FAILURES:
        progress("failure-start " + failure)
        failures.append(experiment.sample(failure, 1, artifact, producer["manifest"]))
    report = build_report(config, samples, failures, producer, True, frozen_sha,
                          {"native_total_job_seconds": time.monotonic() - started,
                           "background_suppression_readback": True, "dedicated_account_empty_at_end": not processes(experiment.uid)})
    write_json(experiment.raw / "report.json", report)
    return report
