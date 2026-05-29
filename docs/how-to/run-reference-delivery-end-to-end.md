# Run the reference delivery end-to-end scenario

This guide runs the L2 happy path: one human-filed issue enters the reference
workflow and the in-process memory world carries it to a merged, reconciled
implementation PR.

## Command

```sh
cargo test -p harness-runner end_to_end
```

The test is `crates/harness-runner/tests/end_to_end.rs`. It runs the shared
`happy_path()` scenario from runner test support on
`InProcessStage<MemoryForge>` with the full fake reference world:

- fake architect, engineer, reviewer, owner, and human agents behind the normal
  `Agent`/`RoleTools` boundary
- `MechanicalWorker` for reconcile → apply
- `CiWorker<MemoryCiSink>` to produce native CI jobs for tests
- `FixpointDriver` with a small fixed tick budget

## Expected path

The fake-driven loop is:

1. seed creates one `untriaged` issue
2. architect runs `triage_to_code`
3. engineer runs `claim_code`, opens an implementation PR through the PR-create
   seam, then runs `request_review`
4. CI worker records a passing native CI job
5. reviewer runs `approve_review` as the requested reviewer
6. owner runs `approve_merge`
7. architect runs `reconcile_landed`
8. `scan` returns no work items, so the world is quiescent

## Accepted seams

The assertion intentionally accepts two current workflow seams:

- the produced code issue can remain open after its PR merges
- the merged PR keeps `alignment`; one PR is below `owner_alignment`'s
  `min_depth` of 5 and is fresh, so that queue should not activate
