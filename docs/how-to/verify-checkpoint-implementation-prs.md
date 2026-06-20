# Verify checkpoint-only implementation PRs end to end

Use this recipe to verify the `temper run` engineer path with no real LLM credentials: the coding agent makes a product edit, optionally calls `checkpoint`, and Temper opens the implementation PR from the final product diff.

The fixture runs against a throwaway Forgejo server and a real host-mode `forgejo-runner`. The LLM endpoint is a local `jig_server::FakeLlm` scripted by the test.

## Prerequisites

- Build and run from the `temper` repository root.
- `forgejo-server` and `forgejo-runner` must be available, or the pinned Forgejo fixture must be able to resolve them into `.cache/forgejo/`.
- The host must allow local child processes, localhost ports, and host-mode Actions jobs.

## Run the focused verification

From a fresh checkout of the revision you want to verify:

```sh
cargo build --bin temper
TEMPER_TEST_CONVERGENCE_TIMEOUT_SECS=180 \
  cargo test --test run_forgejo_e2e \
  temper_run_opens_pr_from_checkpointed_product_diff_via_fake_llm -- --ignored --nocapture
```

`cargo test` builds the test-scoped `temper` binary from the current checkout and the test launches that binary with an isolated config and credentials file. The fake provider URL comes from the fixture config; no real provider environment variables are required.

## What the test proves

The scripted fake engineer performs three turns:

1. Writes `DELIVERY.md` into the prepared checkout.
2. Calls `checkpoint` with `Create delivery file` after the product edit exists.
3. Returns final success JSON with a summary only.

The assertion loop then verifies that the engineer-authored implementation PR is created from the product diff, contains the final summary and workflow metadata, and does not contain a model-authored implementation-plan checklist.

## Cleanup

The fixture kills the spawned `temper` process on drop. If a run is interrupted with `SIGKILL`, clean up orphaned processes and temp trees before retrying:

```sh
pkill -f forgejo || true
pkill -f forgejo-runner || true
pkill -f 'temper daemon' || true
rm -rf /tmp/temper-run-forgejo-e2e-* /tmp/temper-forgejo-*
```

For fixture internals and pinned binary controls, see [Run the daemon end-to-end fixture](run-daemon-e2e.md) and [Forgejo e2e fixture reference](../reference/forgejo-e2e-fixture.md).
