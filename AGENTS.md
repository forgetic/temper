# Agent operating guide

This repository is designed to be evolved by autonomous coding agents. Treat this file as the first source of operational guidance.

## Session bootstrap

1. Confirm you are in the repository root: `/home/free/src/rust/harness`.
2. Read `README.md` for current project status and workspace layout.
3. Read `docs/README.md` to choose the right documentation path.
4. Read `docs/reference/agent-lessons/README.md` and any task-relevant lessons.
5. Read task-relevant reference, explanation, how-to, and ADR files before editing.
6. Use `rg` to discover related code and docs; avoid relying on conversational history.

## Current repository state

- `harness-forge` defines the backend-agnostic Forge domain model and async interface.
- `harness-workflow` is the workflow/orchestration crate; it provides the typed workflow spec (`RawWorkflowSpec`), typed ids, validation diagnostics, and `ValidatedWorkflow` (Phase 2), plus artifact/Forge target mapping, workflow metadata blocks, and a classifier for Forge issues and pull requests (Phase 3), plus compilation of a validated workflow into role/prompt/tool/queue/label manifests and a runtime transition table (Phase 4), plus a pure planner (`plan::Planner`) for queue matching, queue activation, and transition planning into typed `WorkflowEffect`s and postconditions (Phase 5), plus a runtime executor (`execute::Executor`) that loads fresh Forge state, re-plans, applies label transitions through `harness-forge`, verifies postconditions, and supports idempotent issue creation via correlation keys (Phase 6), plus recovery primitives — leases (`lease::LeasePlanner`/`LeaseManager`), command journaling (`journal::CommandJournal`/`InMemoryJournal` and `Executor::execute_journaled`), and a reconciler (`reconcile::Reconciler`/`RecoveryPolicy`) for expired leases, partial transitions, and impossible states (Phase 7), plus deterministic robustness and crash-injection tests — a `CrashForge` fault-injecting `Forge` test wrapper (`tests/support/crash.rs`) plus `tests/crash_injection.rs` and `tests/safety_properties.rs` — that prove the runtime's safety properties (Phase 8), plus expression/planning of non-label assignee, comment, pull-request create, and merge effects for the evolving reference workflow (Phase 9a), execution of assignee/comment effects with `execute::ExecutionContext` role-to-user resolution plus comment idempotency markers (Phase 9b), at-most-once execution of `MergePullRequest` with the post-merge `landed`/`alignment` projection modeled as `add_label` effects (Phase 9c), idempotent pull-request creation via `Executor::ensure_pull_request` plus `CreatePullRequest` execution with runtime create inputs (Phase 10), external-signal gates satisfied by Forge-projected label/state conditions (Phase 11), native CI gates fed from `CiJob` conclusions with derived merge eligibility (ADR 0014), first-class relation declarations plus typed classification of metadata-projected relations (Phase 12a), a relation-driven `dependency_gate` (the `dependencies_resolved` gate condition) with `Planner::dependency_unblocks` and a mechanical reconciler `Unblock` action for `blocked` work (Phase 12b), native dependency links with metadata fallback and Forge-derived dependency status (ADR 0015), queue activation policy via `min_depth`/`max_age` (Phase 13), multi-kind/disjunctive queue matching (Phase 14), native pull-request reviews with review gates (ADR 0016), and CI pass/failure routing from native CI status rather than testing labels (ADR 0017), plus compare-and-swap lease acquisition built on the portable optimistic-concurrency `Version`/`expected_version` primitive: `LeaseManager` captures the artifact version at load and writes leases conditionally (`prepare_acquire`/`commit`), surfacing a lost race as `LeaseError::Contended` (ADR 0013), plus automatic application of reconciler actions via `recover::Applier`, which routes each `RecoveryAction` through the executor's idempotent label-apply path (`Executor::apply_label_effects`), `LeaseManager::clear`, and the command journal — idempotent, crash-safe, and convergent, with `Escalate`/`Diagnose` kept advisory. It depends on `harness-forge` (see ADR 0007, ADR 0010, ADR 0011, ADR 0012, ADR 0013, ADR 0014, ADR 0015, ADR 0016, and ADR 0017); see `docs/reference/robustness-guarantees.md` for the proven properties and known limitations.
- `harness-forge-filesystem` implements local filesystem backend support for users, per-handle identity, repositories, repository labels, issues, native dependency links, issue comments, pull requests, pull-request comments, pull-request reviews, pull-request merges, and CI job listing/lookup plus deterministic CI fixture seeding.
- `harness-forge-memory` implements the same Forge contract in memory (`MemoryForge`), reproducing the filesystem backend's deterministic identifiers, logical clock, ordering, and query semantics; it adds a handle-local identity hook (`MemoryForge::as_user`) plus a one-shot fault hook (`MemoryForge::fail_next`) for testing backend error paths and is the backend the workflow-layer tests run against. Concrete backends follow the `harness-forge-<provider>` naming convention (see ADR 0008).
- `harness-runner` contains backend-agnostic reusable runner primitives: read-only `scan`/`scan_role` functions that turn fresh Forge state into active-queue `WorkItem`s for subscriber roles, plus the production worker-plane primitives `Agent`, `RoleTools`, `Worker`, `RoleWorker`, and `MechanicalWorker`, plus the test-only CI producer seam (`CiSink`/`CiWorker`) used to seed native CI jobs while the engine keeps reading CI through `list_ci_jobs`. `RoleTools` is the only workflow-state mutation path exposed to agents; it wraps authorized transition execution, the idempotent pull-request creation seam, and a narrow native issue-close tool for workflow-specific lifecycle projection. `MechanicalWorker` is the controller-plane worker that runs reconcile → apply ticks for expired-lease requeue, partial-transition repair, dependency unblock, and stale-command cleanup without spawning agents. `RunnerConfig`, `FixpointDriver`, `PollLoop`, `Scenario`/`Stage`, `InProcessStage`, and `MultiProcessStage` compose workers into process-layout-independent runnable worlds for layered tests and future production glue; runner integration-test support now provides deterministic behavior-only fake agents plus `MemoryCiSink` and `FilesystemCiSink` for the reference-delivery world, the L2/L3 happy path plus review-failure, CI-failure, and dependency-unblock variants prove that the same scenarios reach merged, reconciled PRs on `MemoryForge` and `FilesystemForge`, and the happy path also runs through the distinct-handle filesystem process-split sketch.
- Documentation follows Diátaxis and is part of the product, not an afterthought.
- Agent lessons live in `docs/reference/agent-lessons/` so corrections survive across sessions.
- The intended real-world Forgejo deployment is webhook-accelerated and poll-backstopped, with triggering kept off the `harness-forge` trait; see ADR 0009 and `docs/explanation/agentic-workflows.md` (Triggering model). `PollLoop` is now the level-triggered backstop for one worker; the remaining follow-up in this direction is introducing the `ChangeHint` abstraction plus an optional `ChangeSource` companion trait as a latency accelerator.
- The reference-workflow backlog (Phases 9a–14) is complete, lease acquisition is now compare-and-swap (ADR 0013), and reconciler actions are now applied automatically: `recover::Applier` routes each `RecoveryAction` through the executor's idempotent label-apply path, `LeaseManager::clear`, and the command journal, idempotently and crash-safely, so the scan→apply loop converges (`Escalate`/`Diagnose` stay advisory; see `docs/reference/workflow-layer.md` "Applying reconciler actions"). The native-Forge-state backlog has completed Phase A (derived merge eligibility and native CI gates), Phase B (native dependency links with metadata fallback), Phase C (native pull-request reviews), and the ADR 0017 cleanup that routes CI pass/failure from native CI status instead of testing labels; likely next work is adding thin per-worker filesystem binaries for the process split, another concrete Forge backend (the `harness-forge-<provider>` convention and two reference backends are in place), native provider review adapters, or projecting `Escalate`/`Diagnose` into labels/comments.

## Ground rules

- Keep the Forge interface backend-agnostic. Do not leak Forgejo, GitHub, or filesystem-specific concepts into `harness-forge` unless they are modeled as portable concepts.
- Prefer small, documented changes. Future agents should be able to infer intent from committed files alone.
- Every public Forge API change must update `docs/reference/forge-interface.md`.
- Every domain-model change must update `docs/explanation/domain-model.md` when it changes the conceptual model.
- Add or update tests with behavior changes.
- Keep Rust source and test files at or below 600 lines; split focused modules or shared test support before exceeding that budget.
- Run `cargo fmt --all`, `cargo dev-clippy`, and `cargo dev-check` before handing off.

## Documentation expectations

Use Diátaxis deliberately:

- `docs/tutorials/`: guided lessons for newcomers.
- `docs/how-to/`: recipes for specific tasks.
- `docs/reference/`: exact contracts, API semantics, schemas, and invariants.
- `docs/explanation/`: rationale, mental models, and design trade-offs.
- `docs/adr/`: decisions that should remain visible over time.

Keep hand-written documents small for agent context loading: target about 150 lines or fewer, and split documents before they exceed about 350 lines. Prefer short index files that point to focused pages.

Capture repeated mistakes and human steering in `docs/reference/agent-lessons/`. Use `docs/how-to/record-agent-lesson.md` when adding a lesson.

Do not mix tutorial prose into reference pages. Do not hide contracts in explanation pages.

## Rust conventions

- Keep `harness-forge` free of concrete backend dependencies.
- Keep `harness-workflow` focused on workflow and orchestration logic; it is the renamed workflow crate (formerly `harness-core`).
- Use typed identifiers instead of raw strings in public APIs.
- Prefer explicit state enums over stringly-typed statuses.
- Keep async boundaries at the backend interface; concrete backends may use sync internals if appropriate.
- Avoid global mutable state.

## Fast local iteration

- Use `cargo dev-check` for the default validation loop; it checks the whole workspace and all targets without producing binaries.
- Clippy is installed: use `cargo dev-clippy` (`cargo clippy --workspace --all-targets`) and keep its output clean before handoff.
- Cargo uses all available logical CPU cores by default. Do not set a fixed `build.jobs` value unless the task explicitly requires it.
- Development and test profiles are tuned for fast local compilation in `Cargo.toml`; production profiles can be designed later.

## Backend conventions

Each backend should document:

- Supported operations.
- Persistence model and schema, if any.
- Consistency guarantees.
- Unsupported Forge features and how they fail.

The filesystem and in-memory backends are the reference backends for deterministic tests and fast iteration, not production forges. They must keep the same observable contract (see ADR 0008); a behaviour change to one usually needs the same change in the other, and in both `docs/reference/filesystem-backend.md` and `docs/reference/in-memory-backend.md`.

## Session closeout

Before handing off, follow `docs/how-to/end-a-development-session.md`. Start the review from top-level `README.md` and `AGENTS.md`, then update any task-relevant docs so the next agent can continue without hidden context. If the session involved a correction, failed assumption, or missing guidance, record or update an agent lesson.
