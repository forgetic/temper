# Runner process split bridge

This page is the bridge from the current runner tests to the next topology: one
OS process per worker over a shared filesystem-backed Forge store. Phase 08 does
not add those binaries; it leaves the split as an additive step.

## Already reusable

- `Worker` is the process unit. `RoleWorker`, `MechanicalWorker`, and the
  test-only `CiWorker` all advance by one `tick(now)` and coordinate only by
  reading or mutating Forge state.
- `RoleTools` is already the role-scoped state mutation boundary for fake and
  future LLM agents.
- `RunnerConfig` carries repository, role→user, PR-create, lease, and poll
  cadence settings without assuming a process layout.
- `FixpointDriver` remains the deterministic in-process scheduler for L2/L3.
- `PollLoop` now drives one worker on a poll cadence. `run_until` is the
  production entry point each per-process binary will run; `run_bounded` gives
  tests a no-sleep deterministic form of the same primitive. Both have
  deterministic single-worker tests, including the `run_until` post-tick
  shutdown check that avoids waiting an extra interval.
- `Stage`/`Scenario` keep seed and assertions backend/topology-neutral.
- `MultiProcessStage` is the L4 rehearsal: it still runs in one test process, but
  each worker is built from its own Forge handle. The filesystem test factory
  creates fresh `FilesystemForge::new(root)` handles over the same directory and
  applies `as_user` for role identities.
- `FilesystemForge::as_user` and `MemoryForge::as_user` provide the per-handle
  identity seam. The override is handle-local and not durable state.

## What the next phase adds

1. Thin per-worker binaries:
   - `role-worker`: load `RunnerConfig`, construct an authenticated
     `FilesystemForge` handle for one role, build its `RoleWorker`, run
     `PollLoop::run_until`.
   - `mechanical`: load the same config, construct the controller identity, build
     `MechanicalWorker`, run a `PollLoop`.
   - optional test/local `ci`: build `CiWorker` only for filesystem rehearsal;
     production gets CI from the provider.
2. A launcher or fixture that starts those binaries against one repository
   directory. No runner primitive should need rewriting.
3. Real CI producer replacement: Forgejo Actions writes native CI state; the
   engine already reads it through `list_ci_jobs`, so runner production code does
   not need `CiSink` or fake CI.
4. L4 crash/restart and contention tests become meaningful once workers are real
   processes:
   - kill a role worker after claiming a lease; mechanical requeues after TTL.
   - kill during a multi-effect transition; reconciler repairs partial labels.
   - start two role workers for one queue; CAS lease acquisition permits one
     winner and leaves the loser harmless.
   - restart the mechanical process with an empty journal; Forge state still
     converges.
   - vary poll intervals to prove level polling eventually observes missed edge
     hints.
5. Future production adapters remain isolated behind existing traits:
   - `harness-forge-forgejo` implements `Forge` for Forgejo.
   - an LLM adapter implements `Agent` and uses `RoleTools`.
   - a webhook adapter emits `ChangeHint`s through a future `ChangeSource`; the
     poll loop stays the correctness backstop from ADR 0009.

## Current limitation

`MultiProcessStage` is not OS isolation. It proves separate handles and
forge-only coordination in one process; process crashes, restart supervision,
and true concurrent filesystem writes belong to the next phase. The filesystem
backend currently documents atomic single-record writes but no file locks, so L4
contention tests should first add or explicitly scope the write-locking story.
