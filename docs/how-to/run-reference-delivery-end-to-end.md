# Run the reference delivery end-to-end scenarios

This guide runs the reference-delivery scenario suite at the current layered
boundaries: L2 on `MemoryForge`, L3 on `FilesystemForge`, and the happy path on
the L4 `MultiProcessStage` sketch with distinct filesystem handles. The same
scenario definitions seed and assert each backend/topology. For the
operator-facing multi-repo worker-pool demo, see
`examples/reference-delivery/README.md`.

## Command

```sh
cargo test -p temper-runner --test end_to_end
```

The tests live in `crates/temper-runner/tests/end_to_end.rs`. They run shared
scenarios from runner test support on:

- `InProcessStage<MemoryForge>` with per-role `as_user` handles and
  `CiWorker<MemoryCiSink>`
- `InProcessStage<FilesystemForge>` rooted in a unique temp dir, with per-role
  `as_user` handles and `CiWorker<FilesystemCiSink>` seeding `ci_jobs.json`
- `MultiProcessStage<FilesystemForge>` for the happy path, creating a fresh
  filesystem handle per worker over the same repo directory
- fake architect, engineer, reviewer, owner, and human agents behind the normal
  `Agent`/`RoleTools` boundary
- `MechanicalWorker` for reconcile → apply
- `FixpointDriver` with small fixed tick budgets; `PollLoop::run_bounded` is
  covered separately as the no-sleep one-worker cadence primitive

## Scenarios

- `happy_path()` — one untriaged issue becomes one merged, reconciled PR.
- `changes_requested_then_approved()` — the reviewer requests changes first;
  the engineer returns the PR to review and a later approval permits merge.
- `ci_fails_then_passes()` — fake CI records failure first; the engineer handles
  the CI-failed queue and the fake producer records a later passing verdict.
- `dependency_chain_mechanically_unblocked()` — code issue B depends on code
  issue A; after A lands and is closed, the mechanical worker removes `blocked`,
  adds `ready`, and B proceeds to its own merged PR.
- `cross_repo_fanout_converges()` — one parent intake in repo A creates child
  code issues in repos A and B; both child PRs merge before the parent resolves.

## Accepted seams

- Merging a produced PR does not automatically close its parent code issue. The
  dependency variant uses a closing fake architect on `reconcile_landed` to make
  that native issue lifecycle projection explicit for the dependency gate. That
  close also clears `in-progress`, so completed code issues do not remain
  labelled as active work.
- Merged PRs keep `alignment` until `owner_alignment` activates. The end-to-end
  cohorts are below that queue's `min_depth` of 5 and are fresh, so the queue
  should not activate.
- CI production is test-only. The workflow still reads native CI through the
  Forge API; the fake CI policy only decides which test verdicts to seed.

## Related

For the real-backend, real-process proof of the production daemon topology —
the real `temper-daemon` binary plus a wire-protocol worker against a
throwaway Forgejo with real CI — see [run-daemon-e2e.md](run-daemon-e2e.md).
