# Run the multi-process end-to-end rehearsal

This guide runs the **true one-process-per-part** rehearsal of the
reference-delivery workflow. Unlike the in-process `MultiProcessStage` sketch —
which still runs every worker inside one OS process — these tests spawn the
`temper-testing-worker` binary once per moving part and let the parts
coordinate only through a shared on-disk `FilesystemForge` store.

## Command

```sh
cargo test -p temper-testing --test multiprocess
```

The single-repo scenario tests live in `crates/temper-testing/tests/multiprocess.rs`.
They are part of the default `cargo dev-test` suite because they are fast enough
for normal local iteration.

## Multi-repo fixed worker set

Phase 4 of the multi-repo worker plan adds a focused two-repository rehearsal:

```sh
cargo test -p temper-testing --test multi_repo_multiprocess
```

That test provisions `acme/service-alpha` and `acme/service-beta` in one shared
filesystem store, starts one role worker per role plus one mechanical worker and
one fake CI producer, and passes both `--repo` values to every child. On timeout
it reports the stalled repository path next to the last scenario assertion error.

## Scenarios

All five reference-delivery scenarios run across real processes. Each reuses its
**exact** in-process seed/assert closures (`temper_testing::scenarios`); only
the spawned worker *behavior* differs, selected with `temper-testing-worker`
flags that mirror the in-process registry wiring in
`temper-runner/tests/end_to_end.rs`:

| Scenario | `--architect` | `--reviewer` | `--ci` |
| --- | --- | --- | --- |
| happy path | `default` | `default` | `pass` |
| changes requested then approved | `default` | `request-changes-then-approve` | `pass` |
| CI fails then passes | `default` | `default` | `fail-then-pass` |
| dependency chain mechanically unblocked | `closing` | `default` | `pass` |
| cross-repo fan-out converges | `closing` | `default` | `pass` |

The flags only bite for the architect and reviewer role workers and the CI
producer; every other worker ignores them. No assertion logic is forked per
topology — the same closures check both the in-process and multi-process worlds.

## What it does

Each scenario test (`run_variant`):

1. Creates a unique temp store root (cleaned up on drop).
2. Provisions the repository and labels, and seeds the issue — in-process,
   through the same library code and the **exact** scenario seed closure the
   in-process scenarios use.
3. Spawns one OS process per moving part via the root package's
   feature-gated `temper-testing-worker` binary (the test helper builds
   `cargo build -p temper --bin temper-testing-worker --features testing-worker`
   on demand when Cargo did not provide a binary path):
   - one `--kind role` worker for each role-with-an-agent (derived from the
     compiled workflow ∩ registered fake agents ∩ `RunnerConfig` bindings — never
     a hardcoded list), each carrying the scenario's `--architect`/`--reviewer`
     behavior flags,
   - one `--kind mechanical` reconcile/apply worker,
   - one `--kind ci` fake CI producer carrying the scenario's `--ci` policy.

   Each child gets `--root`, `--repo`, a short `--poll-ms`, a shared
   `--stop-file` sentinel, and a `--run-secs` backstop. The child handles live
   behind a kill-on-drop guard so a panic never orphans a process.
4. Detects convergence in-process by polling the **exact** scenario assert
   closure on a short interval until it passes or a generous (30s) wall-clock
   timeout. The tests serialize their worker fleets internally, so the command
   above does not need a serial libtest harness even though each test launches
   many OS processes.
5. Touches the stop sentinel, waits briefly for every child, kills any child
   that does not observe shutdown, asserts each exited `0`, and runs the assert
   once more for a clean failure message.

## What it proves

The architecture's central claim: workers coordinate **solely** through the
Forge and survive real process boundaries plus true parallel OS scheduling. The
driver only ever touches the store to provision, seed, and observe — every
workflow-state transition is made by a separate process. Because each scenario
reuses its in-process seed and assert verbatim, the multi-process world is
checked against the **same** end-state as the single-process world — including
failed-gate return routing (changes requested), CI fail/recover, and
cross-process mechanical unblocking of a dependency chain, and cross-repo fan-out
on one fixed two-repo worker fleet.

## Determinism caveat

This is intentional process-boundary coverage. It spawns real processes and
detects convergence by wall-clock polling, but the filesystem backend and fake CI
keep it fast enough for the default suite. The deterministic in-process scenarios
(see [run-reference-delivery-end-to-end.md](run-reference-delivery-end-to-end.md))
remain the first-line coverage for workflow logic; this test covers the
*topology*.

## Forgejo twin

This rehearsal deliberately stays on fakes so that going live is **wiring, not
redesign**. Everything above the handle factory is `dyn Forge`, and agents only
mutate state through the `Agent`/`RoleTools` boundary.

The Forgejo multi-process e2e target is the same rehearsal with a real Forgejo
backend, real host-mode `forgejo-runner` CI, wall-clock workers, per-role Forgejo
identities, and the production webhook wake path. It reuses the exact scenario
seed/assert closures; only backend, CI, identity, clock, and trigger wiring
change. Run it with
[run-forgejo-multiprocess-e2e.md](run-forgejo-multiprocess-e2e.md) and load
[forgejo-e2e-topology.md](../explanation/forgejo-e2e-topology.md) for the
non-obvious real-backend quirks.
