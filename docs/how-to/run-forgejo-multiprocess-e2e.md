# Run the Forgejo end-to-end fixture

The Forgejo fixture is the real-backend twin of the filesystem
[multi-process rehearsal](run-multiprocess-e2e.md). It runs deterministic fake
agents against throwaway Forgejo servers, real host-mode `forgejo-runner`
processes, real CI, and the production-shaped webhook wake path. Real LLM
process-boundary coverage lives in Smith; see [Real LLM process proof](#real-llm-process-proof).

Everything here is `#[ignore]`d, so default `cargo test` stays hermetic. The
fixture starts real OS processes and executes CI directly on the host; run it
only on a machine where that is acceptable.

For fixture internals, pinned binaries, cache layout, and Forgejo-specific
quirks, read [Forgejo e2e fixture reference](../reference/forgejo-e2e-fixture.md).
For design rationale, read
[Forgejo multi-process e2e topology](../explanation/forgejo-e2e-topology.md).

## Run all scenarios

```sh
cargo test -p temper-testing --test forgejo_multiprocess -- --ignored
```

This runs the ignored Forgejo convergence scenarios in that test target. The
current scenario list is visible in
`crates/temper-testing/tests/forgejo_multiprocess.rs`. Each scenario owns its
server, runner, trigger, repository state copy, wake dir,
stop file, logs, and worker fleet.

First use may download pinned Forgejo and `forgejo-runner` binaries into
`.cache/forgejo/` and publish scenario state caches. Warmed runs copy state from
`.cache` to `/tmp`. Libtest default parallelism is supported; add
`--test-threads=1` only to throttle host CPU/I/O.

## Retry one scenario

```sh
cargo test -p temper-testing --test forgejo_multiprocess \
  forgejo_multiprocess_ci_fails_then_passes_converges -- --ignored
```

Use the same shape with any `forgejo_multiprocess_*_converges` test name.

## Build up with smoke tests

Start smaller when debugging the fixture stack:

```sh
cargo test -p temper-testing --test forgejo_server -- --ignored
cargo test -p temper-testing --test forgejo_runner -- --ignored
cargo test -p temper-testing --test forgejo_provision -- --ignored
cargo test -p temper-testing --test forgejo_pr_prep -- --ignored
```

What they prove:

- `forgejo_server` boots a cached throwaway server and reads `/api/v1/version`.
- `forgejo_runner` registers a host-mode runner and observes a real failing CI
  status.
- `forgejo_provision` creates the reference-delivery org, role users/tokens,
  repo, labels, and CI workflow.
- `forgejo_pr_prep` proves the Forgejo-only head-branch prep required before a
  PR can be opened.

## Webhook wake regressions

Single repo:

```sh
cargo test -p temper-testing --test forgejo_webhook_wakeup -- --ignored
```

Multi repo with one fixed worker set:

```sh
cargo test -p temper-testing --test forgejo_multi_repo_webhook -- --ignored
```

These register the production `/forgejo/webhook` trigger and fake-agent Forgejo
workers with authenticated Unix wake sockets. The normal poll backstop is long
(`120000` ms), so convergence before that interval proves webhook-narrowed wake
scans. Mechanical wake scans intentionally visit all configured repositories for
cross-repo recovery; role-worker wakes should narrow to hinted configured repos.

## Real LLM process proof

Temper's Forgejo suite uses deterministic fake agents. To validate real Forgejo
+ real LLM + Temper's process adapter, run Smith's ignored process-boundary e2e
from the Smith checkout after its documented provider/auth preflight:

```sh
cd ~/src/rust/smith
TEMPER_FORGEJO_E2E=1 TEMPER_FORGEJO_AGENTS=1 \
  cargo test -p smith-temper-agent-cli --test forgejo_workflow_role_e2e -- \
  --ignored
```

Add `--test-threads=1` only if you need to throttle host load.

## CPU and cleanup

The throwaway Forgejo web server can consume sustained CPU under this workload.
Temper caps `GOMAXPROCS` for spawned server and runner processes by default
(`TEMPER_FORGEJO_GOMAXPROCS=2`; set it empty to opt out).

If a run is killed with `SIGKILL`, Rust drop guards do not run. Clean up orphaned
processes and temp trees before retrying:

```sh
pkill -f forgejo || true
pkill -f temper-testing-worker || true
rm -rf /tmp/temper-forgejo-*
```

## Running it in CI

Keep this suite separate from the default hermetic job. A dedicated job should:

1. allow pinned-binary resolution or pre-stage binaries with
   `TEMPER_FORGEJO_BINARY` and `TEMPER_FORGEJO_RUNNER_BINARY`;
2. run on a host that permits child processes, localhost ports, and host-mode CI
   jobs (no containers required by the runner label);
3. invoke:

   ```sh
   cargo test -p temper-testing --test forgejo_multiprocess -- --ignored
   ```

Treat it as periodic or on-demand real-backend coverage rather than the default
merge gate. The `forgejo_runner` smoke test is a cheaper preflight for host
compatibility.

## Reading failures

Successful scenario runs print world timing, scenario timing, worker scan
summaries, and per-repo CI diagnostics. Timeout panics include stalled assertion
text, trigger URL, worker log tails, runner log tail, and CI diagnostics.

When debugging slow or stuck runs, first check that webhook wake ticks are
present, status-poll ticks are limited to CI-reading roles, and broad scans are
only the mechanical worker's intentional recovery scans.
