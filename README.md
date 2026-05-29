# Harness

Harness is a Rust workspace for building agentic workflows on top of Forge-like collaboration platforms.

The project is intentionally backend-agnostic. The `harness-forge` crate defines the Forge domain model and abstract interface for repositories, users, issues, pull requests, native dependency links, comments, labels, reviews, merges, and CI jobs. Concrete backends can adapt that interface to local files, in-memory stores, Forgejo, GitHub, or other systems, following the `harness-forge-<provider>` naming convention (see ADR 0008). The `harness-workflow` crate is the workflow/orchestration layer; it now contains the typed workflow spec, static validation, artifact/metadata classification of Forge issues and pull requests, first-class relation declarations, compilation to role/prompt/tool/queue/label manifests, pure queue evaluation with multi-kind/disjunctive matching, queue activation, and transition planning, external/runtime-signal gates including native CI and reviews, a runtime executor that applies planned transitions through the `Forge` trait, and recovery primitives — leases, command journaling, and a reconciler (Phases 2–7) — plus deterministic robustness and crash-injection tests that prove the runtime's safety properties (Phase 8). The `harness-runner` crate has begun collecting reusable production runner primitives; its first primitive is a read-only Forge scan from active queues to role-addressed work items.

## Status

This repository is at the design-first scaffold stage. Expect the public interface to evolve, but document every change as if future agents will maintain the project without conversational context.

Current state:

- `harness-forge` contains the provider-neutral Forge domain model and async interface.
- `harness-workflow` implements the workflow spec types (`RawWorkflowSpec`), typed ids, validation diagnostics, and `ValidatedWorkflow` (Phase 2), plus artifact/Forge mapping, workflow metadata blocks, and a classifier that turns Forge issues and pull requests into typed artifacts or classification diagnostics (Phase 3), plus compilation of a validated workflow into role, prompt, tool, queue, and label manifests and a runtime transition table (Phase 4), plus a pure planner that matches classified artifacts against queues, evaluates queue activation, and plans transitions into typed effects and postconditions without touching a backend (Phase 5), plus a runtime executor (`Executor`) that loads fresh Forge state, re-plans, applies label transitions through the `Forge` trait, verifies postconditions, and supports idempotent issue creation via correlation keys (Phase 6), plus recovery primitives: leases (`LeasePlanner`/`LeaseManager`), command journaling (`CommandJournal`/`InMemoryJournal` with a journaled executor), and a reconciler (`Reconciler`/`RecoveryPolicy`) that scans artifacts and journal entries for expired leases, partial transitions, and impossible states (Phase 7), plus deterministic robustness and crash-injection tests (a `CrashForge` fault-injecting `Forge` wrapper) that prove the runtime's safety properties (Phase 8), plus expression/planning of non-label assignee, comment, pull-request create, and merge effects for the evolving reference workflow (Phase 9a), execution of assignee and comment effects with runtime role-to-user resolution plus comment idempotency markers (Phase 9b), at-most-once execution of `MergePullRequest` with the post-merge `landed`/`alignment` projection modeled as `add_label` effects (Phase 9c), idempotent pull-request creation via `Executor::ensure_pull_request` plus `CreatePullRequest` execution with runtime create inputs (Phase 10), external-signal gates satisfied by Forge-projected label/state conditions (Phase 11), native CI gates fed from `CiJob` conclusions with derived merge eligibility (ADR 0014), first-class relation declarations plus typed classification of metadata-projected relations (Phase 12a), a relation-driven `dependency_gate` (the `dependencies_resolved` gate condition) with a mechanical reconciler unblock for `blocked` work (Phase 12b), native dependency links with metadata fallback and Forge-derived dependency status (ADR 0015), queue activation policy via `min_depth`/`max_age` (Phase 13), multi-kind/disjunctive queue matching (Phase 14), native pull-request reviews with review gates (ADR 0016), and CI pass/failure routing from native CI status rather than testing labels (ADR 0017). It depends on `harness-forge` for the domain types it classifies and executes against. See ADR 0007, ADR 0010, ADR 0011, ADR 0012, ADR 0014, ADR 0015, ADR 0016, ADR 0017, and `docs/reference/robustness-guarantees.md`.
- `harness-forge-filesystem` implements filesystem backend support for users, repositories, repository labels, issues, native dependency links, issue comments, pull requests, pull-request comments, pull-request reviews, pull-request merges, and CI job listing/lookup.
- `harness-forge-memory` implements the same Forge contract entirely in memory. It reproduces the filesystem backend's deterministic identifiers, logical clock, ordering, and query semantics, adds a one-shot fault hook for testing backend error paths, and is the backend the workflow-layer tests run against.
- `harness-runner` contains backend-agnostic runner primitives. It currently provides read-only `scan`/`scan_role` functions that list Forge issues and pull requests, classify them, read public workflow `GateSignals`, apply queue activation, and emit one `WorkItem` per active queue member and subscriber role.
- The reference-workflow backlog (Phases 9a–14) is complete, and lease acquisition is now a portable compare-and-swap: issues and pull requests carry an optimistic-concurrency `Version`, `UpdateIssue`/`UpdatePullRequest` take an `expected_version` precondition, and `LeaseManager` writes leases conditionally so two "no lease" acquirers cannot both win (ADR 0013). Reconciler actions are now applied automatically: `recover::Applier` routes each `RecoveryAction` through the executor's idempotent label-apply path, `LeaseManager::clear`, and the command journal, idempotently and crash-safely, so the scan→apply loop converges (`Escalate`/`Diagnose` stay advisory). The native-Forge-state backlog has completed Phase A (derived merge eligibility and native CI gates), Phase B (native dependency links with metadata fallback), Phase C (native pull-request reviews), and the ADR 0017 cleanup that routes CI pass/failure from native CI status instead of testing labels; likely next work is adding another concrete Forge backend, native review-provider adapters, or projecting `Escalate`/`Diagnose` into labels/comments. See `docs/reference/robustness-guarantees.md`, `docs/explanation/reference-workflow-roadmap.md`, and `docs/explanation/native-forge-state-roadmap.md`.

## Workspace layout

```text
crates/
  harness-forge/            Domain types and the backend-agnostic Forge interface.
  harness-workflow/         Workflow/orchestration crate (spec, validation, classification, relations, compilation, planning, execution, leases, journaling, reconciliation).
  harness-forge-filesystem/ Local filesystem backend used for fast development and tests.
  harness-forge-memory/     In-memory backend used for fast workflow tests and local runs.
  harness-runner/           Reusable backend-agnostic runner primitives such as read-only queue scanning.
docs/
  tutorials/      Learning-oriented walkthroughs.
  how-to/         Task-oriented recipes.
  reference/      Precise API and behavior contracts.
  explanation/    Concepts, rationale, and trade-offs.
  adr/            Architecture decision records.
```

## Documentation model

Documentation follows [Diátaxis](https://diataxis.fr/): tutorials, how-to guides, reference, and explanation. Start with `docs/README.md`.

Agent learning from past mistakes is captured in `docs/reference/agent-lessons/`; read its index during session bootstrap.

## Development quick start

Use the fast local-development aliases from `.cargo/config.toml`:

```sh
cargo dev-check
cargo fmt --all
```

`cargo dev-check` expands to `cargo check --workspace --all-targets`. Cargo uses all available logical CPU cores by default; keep that default for agent development.

When changing behavior or public interfaces, update the relevant documentation in the same change. At the end of a session, follow `docs/how-to/end-a-development-session.md`.
