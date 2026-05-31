# Lesson 0009: Cap (and clean up) the throwaway Forgejo's CPU in the e2e

## Tags

`forgejo`, `ci`, `testing`, `tooling`, `process`

## Trigger

A human interrupted a real-agent Forgejo e2e run because the spawned `forgejo`
process sat at **over 200% CPU for minutes** (not a brief spike), saturating a
small dev box. They asked future runs to detect a recurrence and work around it.

## What went wrong

The throwaway Forgejo web server (`harness_testing::forgejo_server`) — a Go
program — drives the host to **2+ cores of sustained CPU** under the
multi-process polling workload (5 role workers polling every 500 ms + actions +
git). It is the *server* that spins, not the workers or the runner (they idle
near 0–1%). Two traps:

- `taskset -cp <pid>` does **not** contain it: it re-pins only the process's main
  thread, while Go spreads goroutines across `GOMAXPROCS` OS threads that keep
  running on every core. The process still climbed past 200%.
- Force-killing the test (SIGKILL) orphans the server, runner, and worker
  children — the Rust `Drop` guards only run on a clean exit.

## Steering for future agents

- The harness now caps `GOMAXPROCS` (default `2`) on the spawned Forgejo **and**
  `forgejo-runner` (`forgejo_server/mod.rs::apply_cpu_cap`). Keep that cap; it
  bounds the Go runtime at the source. Override per-run with
  `HARNESS_FORGEJO_GOMAXPROCS` (empty string opts out).
- Run the gated Forgejo e2e tests **serially** (`--test-threads=1`): each boots
  its own ~2-core Forgejo, so parallel tests multiply the load.
- When monitoring, sample `ps`/`taskset -acp` (all threads) to pin forgejo to a
  core subset and keep cores free; do not rely on `taskset -cp`.
- After any force-kill, clean up orphans: `pkill -f forgejo`,
  `pkill -f harness-testing-worker`, and `rm -rf /tmp/harness-forgejo-*`.
- `ps pcpu` is a **lifetime average**, not instantaneous — it lags; treat a
  steadily climbing average toward N×100% as a sustained-load signal.

## Where this is now documented

- `crates/harness-testing/src/forgejo_server/mod.rs` (`apply_cpu_cap`,
  `forgejo_gomaxprocs`, `DEFAULT_FORGEJO_GOMAXPROCS`) + `runner.rs`.
- `docs/how-to/run-forgejo-multiprocess-e2e.md` ("Real LLM agents" / "CPU note").
- `plans/forgejo-e2e/findings-phase-b.md` ("The sustained-CPU incident").
