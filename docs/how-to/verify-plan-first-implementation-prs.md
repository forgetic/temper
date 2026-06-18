# Verify plan-first implementation PRs end to end

Use this recipe to verify the issue #166 flow with no real LLM credentials: the
coding agent publishes a plan through the `publish_plan` tool, Temper opens the
implementation PR before product edits, the fake engineer checkpoints the first
phase, and final success reuses the same PR.

The fixture runs against a throwaway Forgejo server and a real host-mode
`forgejo-runner`. The LLM endpoint is a local `jig_server::FakeLlm` scripted by
the test.

## Prerequisites

- Build and run from the `temper` repository root.
- `forgejo-server` and `forgejo-runner` must be available, or the pinned
  Forgejo fixture must be able to resolve them into `.cache/forgejo/`.
- The host must allow local child processes, localhost ports, and host-mode
  Actions jobs.

## Run the focused verification

From a fresh checkout of the revision you want to verify:

```sh
cargo build --bin temper
TEMPER_TEST_CONVERGENCE_TIMEOUT_SECS=180 \
  cargo test --test run_forgejo_e2e \
  temper_run_publishes_plan_first_pr_via_fake_llm -- --ignored --nocapture
```

`cargo test` builds the test-scoped `temper` binary from the current checkout
and the test launches that binary with an isolated config and credentials file.
The fake provider URL comes from the fixture config; no real provider
environment variables are required.

## What the test proves

The scripted fake engineer performs four turns:

1. Calls `publish_plan` with two phases: `Create delivery file` and
   `Verify delivery`.
2. Polls the throwaway Forgejo API until the implementation PR exists with the
   workflow metadata, `implementation` + `in-progress` labels, and an unchecked
   two-item checklist; only then does it write `DELIVERY.md`.
3. Calls `checkpoint` with `Create delivery file`.
4. Returns final success JSON with the same plan.

The assertion loop then verifies that the final PR is still the same
engineer-authored implementation PR, contains the final summary, and preserves
`Create delivery file` as checked while `Verify delivery` remains unchecked.

A successful run prints output similar to:

```text
run_forgejo_e2e world up: cache_hit=true runner=true startup=...
run_forgejo_e2e seeded intake issue #1
run_forgejo_e2e fake observed plan-first PR before write
run_forgejo_e2e converged: PR #2 authored by UserId("engineer") in ...
test temper_run_publishes_plan_first_pr_via_fake_llm ... ok
```

## Cleanup

The fixture kills the spawned `temper` process on drop. If a run is interrupted
with `SIGKILL`, clean up orphaned processes and temp trees before retrying:

```sh
pkill -f forgejo || true
pkill -f forgejo-runner || true
pkill -f 'temper daemon' || true
rm -rf /tmp/temper-run-forgejo-e2e-* /tmp/temper-forgejo-*
```

For fixture internals and pinned binary controls, see
[Run the daemon end-to-end fixture](run-daemon-e2e.md) and
[Forgejo e2e fixture reference](../reference/forgejo-e2e-fixture.md).
