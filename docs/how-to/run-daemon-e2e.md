# Run the daemon end-to-end fixture

The daemon e2e suite proves the consolidated daemon topology against a real
backend: the real `temper-daemon` binary (env→config→composition, webhook
route, role-token routing, poll + mechanical backstops) plus a deterministic
wire-protocol worker (`temper-testing-daemon-worker`, the `smith-worker`
stand-in) against a throwaway Forgejo server with a real host-mode
`forgejo-runner` producing CI.

Everything here is `#[ignore]`d, so default `cargo test` stays hermetic. The
fixture starts real OS processes and executes CI directly on the host; run it
only on a machine where that is acceptable.

For fixture internals, pinned binaries, cache layout, and Forgejo-specific
quirks, read [Forgejo e2e fixture reference](../reference/forgejo-e2e-fixture.md).
For design rationale, read
[Daemon e2e topology](../explanation/forgejo-e2e-topology.md).

## Run both scenarios

```sh
cargo test --test daemon_forgejo_e2e -- --ignored
```

The test target lives in the root package (it spawns the root
`temper-daemon` `[[bin]]` via `CARGO_BIN_EXE_temper-daemon`). Two scenarios:

- `daemon_forgejo_happy_path_converges` — one seeded intake issue converges to
  a merged implementation PR: the daemon's mechanical backstop stamps the
  issue `code`+`ready`, the worker pushes the branch as the engineer git
  identity, the daemon opens the PR **as the engineer role identity**
  (per-role token routing), real CI goes green, the mechanical backstop merges,
  and the source issue closes through the provider's native close-on-merge
  keyword.
- `daemon_forgejo_ci_fails_then_passes_converges` — the worker's first head
  omits the CI sentinel so real CI fails; the PR must stay unmerged while red;
  a sentinel fix commit turns CI green and the mechanical backstop lands it.

Both assert convergence before the daemon's deliberately long poll-backstop
cadence, so progress is webhook-driven (with the short mechanical cadence as
the CI-status backstop — Forgejo 7.0.x emits no Actions-completion webhooks).

First use may download pinned Forgejo and `forgejo-runner` binaries into
`.cache/forgejo/` and publish scenario state caches. Warmed runs copy state
from `.cache` to `/tmp`.

## Retry one scenario

```sh
cargo test --test daemon_forgejo_e2e \
  daemon_forgejo_ci_fails_then_passes_converges -- --ignored
```

The worker binary is built on demand from the `temper-testing` package when it
is not already in the target directory; set `TEMPER_TESTING_DAEMON_WORKER_BIN`
to point at an explicit binary instead.

## Build up with smoke tests

Start smaller when debugging the fixture stack:

```sh
cargo test -p temper-testing --test forgejo_server -- --ignored
cargo test -p temper-testing --test forgejo_runner -- --ignored
cargo test -p temper-testing --test forgejo_provision -- --ignored
cargo test -p temper-testing --test forgejo_pr_prep -- --ignored
cargo test -p temper-testing --test daemon_worker
```

What they prove:

- `forgejo_server` boots a cached throwaway server and reads `/api/v1/version`.
- `forgejo_runner` registers a host-mode runner and observes a real failing CI
  status.
- `forgejo_provision` creates the reference-delivery org, role users/tokens,
  repo, labels, and CI workflow.
- `forgejo_pr_prep` proves the Forgejo-only head-branch prep required before a
  PR can be opened.
- `daemon_worker` (hermetic, not ignored) proves the deterministic worker's
  wire-protocol contract against an in-process daemon and a `file://` origin.

## Timeouts

The convergence budget is `TEMPER_TEST_CONVERGENCE_TIMEOUT_SECS` (default
300). Raise it on slow hosts rather than editing the tests.

## CPU and cleanup

The throwaway Forgejo web server can consume sustained CPU under this
workload. Temper caps `GOMAXPROCS` for spawned server and runner processes by
default (`BENCH_FORGEJO_GOMAXPROCS=2`; the legacy alias
`TEMPER_FORGEJO_GOMAXPROCS` is still supported; set it empty to opt out).

If a run is killed with `SIGKILL`, Rust drop guards do not run. Clean up
orphaned processes and temp trees before retrying:

```sh
pkill -f forgejo || true
pkill -f temper-daemon || true
pkill -f temper-testing-daemon-worker || true
rm -rf /tmp/temper-daemon-forgejo-e2e-* /tmp/temper-forgejo-*
```

## Running it in CI

Keep this suite separate from the default hermetic job (the repo's
`cargo dev-test-full` alias runs it via `--include-ignored`). A dedicated job
should:

1. allow pinned-binary resolution or pre-stage binaries with
   `BENCH_FORGEJO_BINARY` and `BENCH_FORGEJO_RUNNER_BINARY` (or the legacy
   `TEMPER_FORGEJO_*` aliases);
2. run on a host that permits child processes, localhost ports, and host-mode
   CI jobs (no containers required by the runner label);
3. invoke:

   ```sh
   cargo test --test daemon_forgejo_e2e -- --ignored
   ```

The `forgejo_runner` smoke test is a cheaper preflight for host compatibility.

## Reading failures

Successful runs print world timing, the seeded issue number, and convergence
timing. Timeout panics include the stalled assertion text, the runner log
tail, the daemon log tail, the worker log tail, and per-PR CI diagnostics.

When debugging slow or stuck runs: check the daemon log for webhook wake scans
(`enqueue_scanned_role_work` errors), the worker log for assignment and git
push lines, and the CI diagnostics for run verdicts keyed by head SHA.
