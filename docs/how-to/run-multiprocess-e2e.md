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
mutate state through the `Agent`/`RoleTools` boundary, so each swap is local.

The Forgejo multi-process e2e (`plans/forgejo-e2e/`) executed the **backend** and
**CI** swaps against a real Forgejo server + real host-mode `forgejo-runner` — see
[run-forgejo-multiprocess-e2e.md](run-forgejo-multiprocess-e2e.md) and
[forgejo-e2e-topology.md](../explanation/forgejo-e2e-topology.md). What it ran
(`tests/forgejo_multiprocess.rs`) is the **same** rehearsal as this one, reusing
the **exact** scenario seed/assert closures, with these two pieces made real:

- **Backend handle factory — ✅ done.** The `harness-testing-worker`
  `--backend forgejo` path builds a `ForgejoForge` from `--base-url` plus a
  per-role token (env `HARNESS_FORGEJO_TOKEN`) instead of `FilesystemForge`.
  Nothing above changed because it is all `dyn Forge`. (`worker_bin/forgejo.rs`.)
- **CI — ✅ done.** A real host-mode `forgejo-runner` is the CI producer; the
  `--kind ci` fake worker is **dropped** on Forgejo. The engine reads real
  verdicts through `list_ci_jobs`, which falls back to a **password/web-UI client**
  on Forgejo 7.0.x (`HARNESS_FORGEJO_USERNAME`/`PASSWORD`; ADR 0019).
- **Clock — ✅ done (for that test).** Forgejo workers run `--clock wall`; the
  deterministic `ManualClock` seam (which keeps `owner_alignment`'s `max_age`
  from mis-firing against epoch-based logical timestamps) is filesystem-only.
- **Identity — ✅ done (for that test).** Provisioning mints a real user + token
  per role and feeds each worker its own, so `current_user` matches the
  role-to-user map the executor authorizes against.

- **Agents — ✅ done (Forgejo topology, `--agents real`).** The fake
  `AgentRegistry` entries (`registry_for`) have a real counterpart
  (`real_registry_for` → `harness-agents`): LLM agents for every role
  implementing the same `Agent<F>` trait, mutating workflow state only through
  `RoleTools` (ChatGPT OAuth default, DeepSeek or Anthropic OAuth opt-in).
  Selected by the worker's `--agents real|fake` flag (default `fake`). The
  double-gated real-agent variants of `forgejo_multiprocess.rs`
  converge all four scenarios with real agents (`plans/forgejo-e2e/` Phase B2).

**Still on fakes — pending:**

- **Triggering.** Add the ADR 0009 webhook/`ChangeHint` accelerator **alongside**
  `PollLoop`, feeding the same pull→classify→plan→execute→reconcile reaction
  path. Both topologies are still `PollLoop`-only; polling stays as the
  level-triggered liveness backstop and webhooks only lower latency (see ADR 0009
  and `docs/explanation/agentic-workflows.md`).

No workflow semantics, scenario, queue, label, or transition definitions change.
