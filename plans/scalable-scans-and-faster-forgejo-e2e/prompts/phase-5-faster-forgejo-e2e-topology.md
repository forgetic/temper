# Phase 5 prompt — Faster Forgejo e2e topology

## Goal

Keep the real-backend coverage but stop paying full Forgejo server + runner
startup/provisioning cost for every scenario where repo-level isolation is
enough. The tests should benefit from Phases 1–4 and also reduce fixture setup
cost.

## Required reading

- Phases 1–4 implementations
- `docs/how-to/run-forgejo-multiprocess-e2e.md`
- `docs/explanation/forgejo-e2e-topology.md`
- `crates/temper-testing/tests/forgejo_multiprocess.rs`
- `crates/temper-testing/tests/support/forgejo_multiprocess.rs`
- `crates/temper-testing/tests/support/forgejo_multi_repo.rs`
- `crates/temper-testing/src/forgejo_server/provision.rs`
- `crates/temper-testing/src/forgejo_server/provision_rest.rs`

## Implementation tasks

1. Measure and record current timings before changing topology. Capture at least
   server+runner startup, provisioning, worker convergence, and teardown for the
   full `forgejo_multiprocess` suite.
2. Design a shared live world for `forgejo_multiprocess`:
   - one `ForgejoServer` and one `ForgejoRunner` for the test binary or grouped
     scenario test;
   - one admin/bootstrap and one role identity set;
   - fresh repository names per scenario for isolation;
   - a second fresh repo only for cross-repo fan-out.
3. Refactor provisioning so role identities can be created once and reused across
   multiple repos. Avoid repeatedly minting same-name tokens or recreating users
   unless the helper is explicitly idempotent.
4. Prefer collapsing the five scenario tests into one ignored serial scenario
   suite if that gives reliable cleanup through normal Rust ownership. If keeping
   separate `#[test]` functions, use a cleanup-safe shared fixture design and
   document why it will not orphan processes on panic.
5. Keep worker fleets per scenario isolated by stop file, logs, and unique repo
   args. Reuse server/runner only.
6. Add timeout diagnostics that include:
   - per-worker scan counts and CI-read counts if available from previous phases;
   - runner log tail;
   - repo-specific CI diagnostics;
   - scenario timing breakdown.
7. Revisit poll intervals/timeouts after Phases 1–4. Lower them only if tests
   remain stable on repeated local runs.

## Tests to add or adjust

- Existing `forgejo_multiprocess` scenarios must still converge against real
  Forgejo + real host-mode runner.
- Add a focused unit test for the refactored provisioning helper proving two
  repos can share one role identity map without reminting conflicting tokens.
- If tests are grouped into one ignored suite, keep scenario names visible in
  panic messages and logs.
- Ensure `Drop` cleanup kills server, runner, and all worker fleets on panic.

## Validation

Run at least:

```sh
cargo fmt --all
cargo test -p temper-testing --test forgejo_multiprocess -- --ignored --test-threads=1
cargo test -p temper-testing --test forgejo_webhook_wakeup -- --ignored --test-threads=1
cargo test -p temper-testing --test forgejo_multi_repo_webhook -- --ignored --test-threads=1
cargo dev-check
```

Run the full command from the original report if time permits:

```sh
cargo test -p temper-testing -- --ignored --test-threads=1
```

## Done when

- The Forgejo multiprocess suite preserves scenario isolation through fresh repos
  but does not reboot/re-register a full server+runner for every scenario.
- Timings are recorded before/after and show a material improvement.
- The run remains robust under panic/timeout cleanup.
- This plan README is updated with Phase 5 status and notable findings.
