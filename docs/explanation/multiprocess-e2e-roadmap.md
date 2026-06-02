# Multi-process end-to-end test: implementation roadmap

This roadmap sequences the work to add a **true multi-process**, filesystem-backed
end-to-end rehearsal of the reference-delivery workflow. It is distinct from the
existing `MultiProcessStage` sketch, which still runs every worker in one OS
process (see its doc comment in `crates/temper-runner/src/stage.rs`). The goal
here is **one OS process per moving part** — each role worker, the mechanical
worker, and the fake CI producer — coordinating only through a shared
`FilesystemForge` store.

It deliberately stays on fakes (filesystem backend, fake agents, fake CI) so that
swapping in real-world pieces (the `temper-forge-forgejo` backend, LLM agents,
provider CI) is **wiring, not redesign**. The "swap to real" list in Phase 5 is
the durable record of exactly what that wiring is.

## Why

Prove the architecture's central claim — workers coordinate solely through the
Forge and survive real process boundaries plus true parallel scheduling — and
build the rehearsal scaffold the real deployment slots into. The in-process
scenarios already prove the workflow logic; this proves the *topology*.

## Where the testing machinery lives

Reusable, non-production testing machinery lands in a new **`temper-testing`**
crate. The maintainer rule: testing machinery reused across crates goes there;
crate-specific test helpers stay local (for example `CrashForge` stays in
`temper-workflow` tests, the Forgejo mock HTTP seam stays in
`temper-forge-forgejo`, and `temper-runner`'s narrow primitive tests keep their
inline fakes). `temper-testing` is **never a normal dependency of a production
crate** — only a dev-dependency or a dependency of other test crates.

## Conventions for every phase

- Write the ADR **first** for the one backend change (Phase 1).
- Land green: `cargo fmt --all`, `cargo dev-clippy`, `cargo dev-check`, tests.
- Docs ≤150 lines (split before 350); Rust source/test files ≤600 lines.
- `reference-delivery.json` is owned by a parallel effort. **Do not change
  workflow semantics here.** Derive the worker set from the compiled workflow and
  `RunnerConfig`; never hardcode role or queue names.
- Determinism split (see "Default-run vs `#[ignore]`" rationale): deterministic
  correctness stays in the default suite; the sleepy, process-spawning rehearsal
  is `#[ignore]`d and run via a how-to / dedicated CI job, matching the existing
  env-gated `#[ignore]`d Forgejo live-test precedent.

## Phases

Status legend: ☐ pending · ☑ done.

- ☑ **1 — Filesystem backend cross-process concurrency (ADR 0018).**
  Today `issues.json`, `pull_requests.json`, and the `metadata.json` clock/ID
  counters are file-granular with atomic temp+`rename`, but there is **no
  cross-process lock** and the temp path is fixed. True parallel writers lose
  updates, duplicate issue/PR numbers, and the per-artifact CAS (ADR 0013) only
  holds within one process. Deliverables: ADR 0018; an `fs2` store-level
  exclusive lock around every mutating read-modify-write-persist in
  `operations.rs`; unique temp filenames in `storage.rs::write_json`; a
  deterministic multi-thread backend test proving no lost updates, no duplicate
  numbers, and a single CAS winner; updated `filesystem-backend.md` (and a note
  in `in-memory-backend.md` that cross-process safety is N/A for the
  single-process backend). Runs by default. Independent and mergeable.

- ☑ **2 — `temper-testing` crate; promote reusable fakes.**
  Create `crates/temper-testing` (lib). Move the reusable reference-delivery
  machinery out of `temper-runner/tests/support` into `temper-testing/src`:
  the fake agents, CI policies and filesystem/memory CI sinks, the `Scenario`
  seed/assert definitions, the `RunnerConfig` builder plus repo/user helpers and
  the fixture loader, and `block_on`. `temper-runner` gains a dev-dependency on
  `temper-testing`; its integration tests `use temper_testing::…` and drop
  `mod support`. (A dev-dependency cycle is allowed; if it causes resolution
  trouble, relocate the broad reference-world tests into `temper-testing`
  instead.) No behavior change: all existing tests pass unchanged.

- ☑ **3 — `temper-testing-worker` binary.**
  Add `[[bin]] temper-testing-worker` to `temper-testing`: a fake worker
  process dispatching on `--kind` (`provision` | `role` | `mechanical` | `ci`).
  It builds a `FilesystemForge::new(--root)` handle (`.as_user` for `role`),
  resolves the repository by path, constructs the matching runner worker
  (`RoleWorker` + fake registry, `MechanicalWorker` + per-process
  `InMemoryJournal`, or `CiWorker` + `FilesystemCiSink` + policy), and runs
  `PollLoop::with_clock(…).run_until(stop)` where `stop` = sentinel file exists
  **or** a `--run-secs` deadline. Default to a deterministic `ManualClock` near
  the backend logical-clock origin, with `--clock wall` for real backends (the
  clock-fidelity seam: `owner_alignment`'s `max_age` would mis-fire if wall-clock
  `now` were compared against epoch-based logical timestamps). Non-zero exit and
  a stderr message on any `WorkerError`. Add a bounded, no-sleep `run_bounded`
  smoke test that the bin wires up. Split arg-parsing / worker construction if a
  file nears 600 lines.

- ☑ **4 — Multi-process happy-path e2e (`#[ignore]`d) + how-to.**
  `temper-testing/tests/multiprocess.rs`: create a temp root → provision repo
  and labels → seed via the existing `happy_path()` seed closure → spawn one
  worker **process** per role-with-an-agent plus `mechanical` plus `ci` via
  `Command::new(env!("CARGO_BIN_EXE_temper-testing-worker"))` behind a
  kill-on-drop guard → poll the `happy_path()` assert closure in-process until it
  passes or a generous timeout → write the stop sentinel → join children and
  assert exit 0 → run the assert once more for a clean message. Mark `#[ignore]`.
  Add `docs/how-to/run-multiprocess-e2e.md`. Reuses the exact in-process scenario
  seed and assert, so the multi-process world is checked against the same
  end-state.

- ☑ **5 — Variants, docs, and the swap-to-real list.**
  Parametrized the multi-process test for `changes_requested_then_approved`,
  `ci_fails_then_passes`, and `dependency_chain_mechanically_unblocked` via worker
  behavior flags (`--reviewer`, `--ci`, `--architect`) plus scenario selection,
  each reusing its in-process seed/assert closures verbatim. Updated `README.md`,
  the repository map docs, and cross-referenced
  `run-reference-delivery-end-to-end.md`. Added the explicit **"to swap fakes for
  real, change only this"** list to `run-multiprocess-e2e.md`: replace the
  `FilesystemForge` handle factory with a `ForgejoForge` one; replace fake agents
  with real LLM agents behind the same `Agent`/`RoleTools` boundary; drop the `ci`
  worker (provider CI is the producer; the engine already reads `list_ci_jobs`);
  pass `--clock wall`; add the ADR 0009 webhook/`ChangeHint` accelerator alongside
  `PollLoop`; supply real `RunnerConfig` bindings and tokens.

## Done definition (complete)

All five phases are ☑. `temper-testing` houses the reusable reference-delivery
fakes; the `temper-testing-worker` binary exists with `--architect`/`--reviewer`/
`--ci` behavior flags and `--clock deterministic|wall`; and the `#[ignore]`d
`tests/multiprocess.rs` converges all four reference-delivery scenarios (happy
path, changes-requested return routing, CI fail/recover, and cross-process
mechanical dependency unblock) to their asserted end states across real OS
processes sharing one `FilesystemForge` store. The default suite stays green and
deterministic. Run the rehearsal with
`cargo test -p temper-testing --test multiprocess -- --ignored`; see
`docs/how-to/run-multiprocess-e2e.md` for how to run it and exactly what to
change to swap in real pieces.

This roadmap is now a completed record; no further phases are planned. Future
work is the swap-to-real wiring itself, tracked where the real backend/agents
land — not here.
