"""Linux account lifetime: subreaper, all-process RSS, inclusive child CPU."""
import ctypes
import os
import resource
import signal
import threading
import time
from pathlib import Path

from contract import Refusal


def processes(uid):
    observed = {}
    for path in Path("/proc").iterdir():
        if not path.name.isdigit():
            continue
        try:
            if path.stat().st_uid != uid:
                continue
            fields = (path / "stat").read_text().rsplit(")", 1)[1].split()
            # /proc stat fields 22=starttime,24=rss; fields[0] is state(3).
            observed[int(path.name)] = {"start": int(fields[19]), "rss": int(fields[21]) * os.sysconf("SC_PAGE_SIZE"),
                                       "user_seconds": (int(fields[11]) + int(fields[13])) / os.sysconf("SC_CLK_TCK"),
                                       "system_seconds": (int(fields[12]) + int(fields[14])) / os.sysconf("SC_CLK_TCK"),
                                       "state": fields[0], "parent": int(fields[1]),
                                       "command": (path / "cmdline").read_bytes().replace(b"\0", b" ").decode(errors="replace")}
        except (OSError, ValueError, IndexError):
            continue
    return observed


def require_unused(uid):
    if uid in (0, os.getuid(), int(os.environ.get("SUDO_UID", "0"))):
        raise Refusal("operator-account-forbidden")
    if processes(uid):
        raise Refusal("dedicated-account-busy")


def subreaper():
    if os.geteuid() != 0 or not Path("/proc").exists():
        raise Refusal("native-requires-linux-root-supervisor")
    # Linux PR_SET_CHILD_SUBREAPER. No daemon launch ABI is used.
    if ctypes.CDLL(None, use_errno=True).prctl(36, 1, 0, 0, 0) != 0:
        raise Refusal("subreaper-unavailable")


class AccountMeter:
    def __init__(self, uid, interval, cache=None):
        self.uid = uid
        self.cache = cache
        self.worker_logs = {}
        self.interval = interval
        self.peak = 0
        self.seen = {}
        self.samples = 0
        self.stop = threading.Event()
        self.before = resource.getrusage(resource.RUSAGE_CHILDREN)
        self.carry_user = self.carry_system = 0.0
        self.started = time.monotonic()
        self.thread = threading.Thread(target=self._sample, daemon=True)
        self.thread.start()

    def reset_phase(self):
        """Subtract CPU already consumed by the retained daemon and its reaped children."""
        self.stop.set()
        self.thread.join()
        previous = resource.getrusage(resource.RUSAGE_CHILDREN)
        live = processes(self.uid)
        if any("--index-worker" in p["command"] for p in live.values()):
            raise Refusal("warm-phase-worker-still-active")
        carry_user = sum(p["user_seconds"] for p in live.values())
        carry_system = sum(p["system_seconds"] for p in live.values())
        prior = {"user_cpu_seconds": previous.ru_utime - self.before.ru_utime + carry_user - self.carry_user,
                 "system_cpu_seconds": previous.ru_stime - self.before.ru_stime + carry_system - self.carry_system,
                 "peak_concurrent_provider_rss_bytes": self.peak}
        self.before, self.carry_user, self.carry_system = previous, carry_user, carry_system
        self.peak, self.samples, self.seen = 0, 0, {}
        self.started = time.monotonic()
        self.stop = threading.Event()
        self.thread = threading.Thread(target=self._sample, daemon=True)
        self.thread.start()
        return prior

    def _sample(self):
        while not self.stop.is_set():
            current = processes(self.uid)
            self.peak = max(self.peak, sum(p["rss"] for p in current.values()))
            self.samples += 1
            for pid, data in current.items():
                self.seen[(pid, data["start"])] = data
            if self.cache is not None:
                for path in (self.cache / "logs").glob(".worker-log-*"):
                    if path.name in self.worker_logs:
                        continue
                    try:
                        with path.open("rb") as source:
                            header = source.read(16384)
                        if b"index.worker.start" in header and b"\n" in header:
                            self.worker_logs[path.name] = header.decode(errors="replace")
                    except OSError:
                        continue
            self.stop.wait(self.interval)

    def finish(self, deadline):
        natural = drain(self.uid, deadline)
        self.stop.set()
        self.thread.join()
        after = resource.getrusage(resource.RUSAGE_CHILDREN)
        user_cpu = after.ru_utime - self.before.ru_utime - self.carry_user
        system_cpu = after.ru_stime - self.before.ru_stime - self.carry_system
        return {"natural_account_exit": natural, "user_cpu_seconds": user_cpu,
                "system_cpu_seconds": system_cpu,
                "cpu_accounting_consistent": min(user_cpu, system_cpu) >= -2 / os.sysconf("SC_CLK_TCK"),
                "peak_concurrent_provider_rss_bytes": self.peak, "rss_sample_count": self.samples,
                "sample_interval_seconds": self.interval, "account_job_seconds": time.monotonic() - self.started,
                "observed_process_count": len(self.seen),
                "observed_daemon_count": sum("--daemon" in d["command"] for d in self.seen.values()),
                "observed_index_worker_count": sum("--index-worker" in d["command"] for d in self.seen.values())}


def reap():
    while True:
        try:
            pid, _, _ = os.wait4(-1, os.WNOHANG)
            if pid == 0:
                return
        except ChildProcessError:
            return


def drain(uid, timeout):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        reap()
        if not processes(uid):
            return True
        time.sleep(0.02)
    # A failed natural lifetime stays failed even when emergency cleanup works.
    for sig in (signal.SIGTERM, signal.SIGKILL):
        current = processes(uid)
        for pid, details in current.items():
            latest = processes(uid).get(pid)
            if latest and latest["start"] == details["start"]:
                try:
                    os.kill(pid, sig)
                except ProcessLookupError:
                    pass
        bound = time.monotonic() + 3
        while time.monotonic() < bound:
            reap()
            if not processes(uid):
                return False
            time.sleep(0.02)
    raise Refusal("dedicated-account-cleanup-incomplete")
