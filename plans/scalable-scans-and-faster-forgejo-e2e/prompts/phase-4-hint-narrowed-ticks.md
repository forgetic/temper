# Phase 4 prompt — Hint-narrowed multi-repo ticks

## Goal

A webhook wake for `owner/repo-a` should not immediately scan `owner/repo-b` in a
multi-repo worker. Hints remain advisory: poll and audit ticks still cover the
configured repository set, and unknown/broad hints fall back safely.

This phase applies to production workers and the Forgejo testing worker path.

## Required reading

- Phases 1–3 implementations
- `crates/temper-runner/src/multi_repo.rs`
- `crates/temper-production/src/worker.rs`
- `crates/temper-production/src/wake.rs`
- `crates/temper-testing/src/worker_bin/forgejo.rs`
- `crates/temper-testing/tests/forgejo_multi_repo_webhook.rs`
- `plans/hint-driven-wakeups/README.md`
- `plans/multi-repo-workers/README.md`

## Implementation tasks

1. Extend `MultiRepoRoleWorker` with a tick method that scans only repositories
   matching known hints. Keep the existing full/hinted-order method for poll and
   broad fallback use.
2. Extend `MultiRepoMechanicalWorker` similarly where safe. If mechanical
   narrowing risks missing cross-repo recovery, document the fallback and keep
   mechanical broad only for unknown/broad hints; otherwise narrow by repo on
   known hints.
3. Update production `DriveWorker::tick_for_wake` so:
   - wake ticks with known repo hints scan only matching repos;
   - wake ticks with no known hints perform a broad scan;
   - poll ticks scan the configured set using the Phase 2 normal scan mode;
   - audit ticks, if implemented here, run at a low frequency and use the Phase
     2 audit mode.
4. Update `temper-testing-worker --backend forgejo` to preserve and use decoded
   wake hints rather than only logging that a wake happened.
5. Add or expose logs/counters showing scanned repository count per tick. This is
   important for debugging and for tests that assert repo B was not scanned on a
   repo A wake.
6. Add CLI/config support for audit interval if Phase 2 did not already wire it.
   Keep defaults conservative and document them.

## Tests to add or adjust

- Multi-repo runner unit test: a hint for repo B ticks repo B only on the wake
  path.
- Production worker/wake test or integration test asserting known hints narrow
  the scanned repo set.
- Forgejo multi-repo webhook test should assert immediate wake progress and, via
  logs/counters, no unnecessary scan of the non-hinted repo for the first wake.
- Regression for unknown-repo hints: they must not be dropped silently; broad
  scan fallback still happens.
- Regression for poll/audit: a repo that receives no webhook still converges via
  periodic scan.

## Validation

Run at least:

```sh
cargo fmt --all
cargo test -p temper-runner
cargo test -p temper-production
cargo test -p temper-testing --test multi_repo_multiprocess
cargo dev-check
```

If the Forgejo binary cache is present, run:

```sh
cargo test -p temper-testing --test forgejo_multi_repo_webhook -- --ignored --test-threads=1
```

## Done when

- Repo-specific wake ticks scan only matching repos in production and testing
  worker paths.
- Broad poll/audit backstops remain documented and tested.
- Logs make scan narrowing visible.
- This plan README is updated with Phase 4 status and notable findings.
