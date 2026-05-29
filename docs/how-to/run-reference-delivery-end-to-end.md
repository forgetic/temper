# Run the reference delivery end-to-end scenarios

This guide runs the L2 reference-delivery scenarios on the in-process memory
world: the happy path plus failure/dependency variants that prove work returns
from failed gates and mechanical unblock participates in the same loop.

## Command

```sh
cargo test -p harness-runner --test end_to_end
```

The tests live in `crates/harness-runner/tests/end_to_end.rs`. They run shared
scenarios from runner test support on `InProcessStage<MemoryForge>` with:

- fake architect, engineer, reviewer, owner, and human agents behind the normal
  `Agent`/`RoleTools` boundary
- `MechanicalWorker` for reconcile → apply
- `CiWorker<MemoryCiSink>` to produce native CI jobs for tests
- `FixpointDriver` with small fixed tick budgets

## Scenarios

- `happy_path()` — one untriaged issue becomes one merged, reconciled PR.
- `changes_requested_then_approved()` — the reviewer requests changes first;
  the engineer returns the PR to review and a later approval permits merge.
- `ci_fails_then_passes()` — fake CI records failure first; the engineer handles
  the CI-failed queue and the fake producer records a later passing verdict.
- `dependency_chain_mechanically_unblocked()` — code issue B depends on code
  issue A; after A lands and is closed, the mechanical worker removes `blocked`,
  adds `ready`, and B proceeds to its own merged PR.

## Accepted seams

- Merging a produced PR does not automatically close its parent code issue. The
  dependency variant uses a closing fake architect on `reconcile_landed` to make
  that native issue lifecycle projection explicit for the dependency gate.
- Merged PRs keep `alignment` until `owner_alignment` activates. The end-to-end
  cohorts are below that queue's `min_depth` of 5 and are fresh, so the queue
  should not activate.
- CI production is test-only. The workflow still reads native CI through the
  Forge API; the fake CI policy only decides which test verdicts to seed.
