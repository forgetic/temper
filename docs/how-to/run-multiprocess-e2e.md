# Run the multi-process end-to-end rehearsal

This guide runs the **true one-process-per-part** rehearsal of the
reference-delivery workflow. Unlike the in-process `MultiProcessStage` sketch —
which still runs every worker inside one OS process — these tests spawn the
`harness-testing-worker` binary once per moving part and let the parts
coordinate only through a shared on-disk `FilesystemForge` store.

## Command

```sh
cargo test -p harness-testing --test multiprocess -- --ignored
```

The tests live in `crates/harness-testing/tests/multiprocess.rs`. They are
`#[ignore]`d, so the default `cargo test` skips them; the `--ignored` flag opts
in.

## Scenarios

All four reference-delivery scenarios run across real processes. Each reuses its
**exact** in-process seed/assert closures (`harness_testing::scenarios`); only
the spawned worker *behavior* differs, selected with `harness-testing-worker`
flags that mirror the in-process registry wiring in
`harness-runner/tests/end_to_end.rs`:

| Scenario | `--architect` | `--reviewer` | `--ci` |
| --- | --- | --- | --- |
| happy path | `default` | `default` | `pass` |
| changes requested then approved | `default` | `request-changes-then-approve` | `pass` |
| CI fails then passes | `default` | `default` | `fail-then-pass` |
| dependency chain mechanically unblocked | `closing` | `default` | `pass` |

The flags only bite for the architect and reviewer role workers and the CI
producer; every other worker ignores them. No assertion logic is forked per
topology — the same closures check both the in-process and multi-process worlds.

## What it does

Each scenario test (`run_variant`):

1. Creates a unique temp store root (cleaned up on drop).
2. Provisions the repository and labels, and seeds the issue — in-process,
   through the same library code and the **exact** scenario seed closure the
   in-process scenarios use.
3. Spawns one OS process per moving part via
   `Command::new(env!("CARGO_BIN_EXE_harness-testing-worker"))`:
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
   timeout.
5. Touches the stop sentinel, `wait()`s every child, asserts each exited `0`, and
   runs the assert once more for a clean failure message.

## What it proves

The architecture's central claim: workers coordinate **solely** through the
Forge and survive real process boundaries plus true parallel OS scheduling. The
driver only ever touches the store to provision, seed, and observe — every
workflow-state transition is made by a separate process. Because each scenario
reuses its in-process seed and assert verbatim, the multi-process world is
checked against the **same** end-state as the single-process world — including
failed-gate return routing (changes requested), CI fail/recover, and
cross-process mechanical unblocking of a dependency chain.

## Determinism caveat

This is the one intentional non-deterministic test. It spawns real processes and
detects convergence by wall-clock polling, so — like the env-gated Forgejo live
smoke tests — it is `#[ignore]`d and excluded from the default suite. The
deterministic in-process scenarios (see
[run-reference-delivery-end-to-end.md](run-reference-delivery-end-to-end.md))
remain the default coverage for the workflow logic; this test covers the
*topology*.

## To swap fakes for real, change only this

This rehearsal deliberately stays on fakes so that going live is **wiring, not
redesign**. Everything above the handle factory is `dyn Forge`, and agents only
mutate state through the `Agent`/`RoleTools` boundary, so the swap is local:

- **Backend handle factory.** Replace `FilesystemForge::new(--root)` in
  `worker_bin::run` with a `ForgejoForge` constructed from a base URL plus a
  per-role API token. Nothing above changes because it is all `dyn Forge`.
- **Agents.** Replace the fake `AgentRegistry` entries (`registry_for`) with real
  LLM agents implementing the same `Agent<F>` trait. They keep mutating workflow
  state only through `RoleTools` — the authorized transition path is unchanged.
- **CI.** Drop the `--kind ci` worker entirely. Provider CI (Forgejo Actions) is
  the real producer, and the engine already reads it through `list_ci_jobs`; the
  fake CI producer exists only because the filesystem backend has no Actions.
- **Clock.** Pass `--clock wall`. A real provider writes wall-clock timestamps,
  so the deterministic `ManualClock` seam (which keeps `owner_alignment`'s
  `max_age` from mis-firing against epoch-based logical timestamps) goes away.
- **Triggering.** Add the ADR 0009 webhook/`ChangeHint` accelerator **alongside**
  `PollLoop`, feeding the same pull→classify→plan→execute→reconcile reaction
  path. Polling stays as the level-triggered liveness backstop; webhooks only
  lower latency (see ADR 0009 and `docs/explanation/agentic-workflows.md`).
- **Identity.** Supply real `RunnerConfig` role→user bindings and the matching
  per-role tokens, so each worker's `current_user` matches the role-to-user map
  the executor authorizes against.

No workflow semantics, scenario, queue, label, or transition definitions change.
