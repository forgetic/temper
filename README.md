# Harness

Harness is a Rust workspace for building agentic workflows on top of Forge-like collaboration platforms.

The project is intentionally backend-agnostic. The `harness-forge` crate defines the Forge domain model and abstract interface for repositories, users, issues, pull requests, comments, labels, merges, and CI jobs. Concrete backends can adapt that interface to local files, in-memory stores, Forgejo, GitHub, or other systems, following the `harness-forge-<provider>` naming convention (see ADR 0008). The `harness-workflow` crate is the workflow/orchestration layer; it now contains the typed workflow spec, static validation, artifact/metadata classification of Forge issues and pull requests, compilation to role/prompt/tool/queue/label manifests, pure queue evaluation and transition planning, a runtime executor that applies planned transitions through the `Forge` trait, and recovery primitives — leases, command journaling, and a reconciler (Phases 2–7) — plus deterministic robustness and crash-injection tests that prove the runtime's safety properties (Phase 8).

## Status

This repository is at the design-first scaffold stage. Expect the public interface to evolve, but document every change as if future agents will maintain the project without conversational context.

Current state:

- `harness-forge` contains the provider-neutral Forge domain model and async interface.
- `harness-workflow` implements the workflow spec types (`RawWorkflowSpec`), typed ids, validation diagnostics, and `ValidatedWorkflow` (Phase 2), plus artifact/Forge mapping, workflow metadata blocks, and a classifier that turns Forge issues and pull requests into typed artifacts or classification diagnostics (Phase 3), plus compilation of a validated workflow into role, prompt, tool, queue, and label manifests and a runtime transition table (Phase 4), plus a pure planner that matches classified artifacts against queues and plans transitions into typed effects and postconditions without touching a backend (Phase 5), plus a runtime executor (`Executor`) that loads fresh Forge state, re-plans, applies label transitions through the `Forge` trait, verifies postconditions, and supports idempotent issue creation via correlation keys (Phase 6), plus recovery primitives: leases (`LeasePlanner`/`LeaseManager`), command journaling (`CommandJournal`/`InMemoryJournal` with a journaled executor), and a reconciler (`Reconciler`/`RecoveryPolicy`) that scans artifacts and journal entries for expired leases, partial transitions, and impossible states (Phase 7), plus deterministic robustness and crash-injection tests (a `CrashForge` fault-injecting `Forge` wrapper) that prove the runtime's safety properties (Phase 8). It depends on `harness-forge` for the domain types it classifies and executes against. See ADR 0007 and `docs/reference/robustness-guarantees.md`.
- `harness-forge-filesystem` implements filesystem backend support for users, repositories, repository labels, issues, issue comments, pull requests, pull-request comments, pull-request merges, and CI job listing/lookup.
- `harness-forge-memory` implements the same Forge contract entirely in memory. It reproduces the filesystem backend's deterministic identifiers, logical clock, ordering, and query semantics, adds a one-shot fault hook for testing backend error paths, and is the backend the workflow-layer tests run against.
- The next likely implementation task is to address the limitations the robustness tests surfaced (compare-and-swap lease acquisition, pull-request idempotent create, applying reconciler actions automatically, non-label transition effects), or to add another concrete Forge backend. See `docs/reference/robustness-guarantees.md`.

## Workspace layout

```text
crates/
  harness-forge/            Domain types and the backend-agnostic Forge interface.
  harness-workflow/         Workflow/orchestration crate (spec, validation, classification, compilation, planning, execution, leases, journaling, reconciliation).
  harness-forge-filesystem/ Local filesystem backend used for fast development and tests.
  harness-forge-memory/     In-memory backend used for fast workflow tests and local runs.
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
