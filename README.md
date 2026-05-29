# Harness

Harness is a Rust workspace for building agentic workflows on top of Forge-like collaboration platforms.

The project is intentionally backend-agnostic. The `harness-forge` crate defines the Forge domain model and abstract interface for repositories, users, issues, pull requests, comments, labels, merges, and CI jobs. Concrete backends can adapt that interface to local files, Forgejo, GitHub, or other systems. The `harness-workflow` crate is the workflow/orchestration layer; it now contains the typed workflow spec, static validation, artifact/metadata classification of Forge issues and pull requests, compilation to role/prompt/tool/queue/label manifests, pure queue evaluation and transition planning, and a runtime executor that applies planned transitions through the `Forge` trait (Phases 2–6).

## Status

This repository is at the design-first scaffold stage. Expect the public interface to evolve, but document every change as if future agents will maintain the project without conversational context.

Current state:

- `harness-forge` contains the provider-neutral Forge domain model and async interface.
- `harness-workflow` implements the workflow spec types (`RawWorkflowSpec`), typed ids, validation diagnostics, and `ValidatedWorkflow` (Phase 2), plus artifact/Forge mapping, workflow metadata blocks, and a classifier that turns Forge issues and pull requests into typed artifacts or classification diagnostics (Phase 3), plus compilation of a validated workflow into role, prompt, tool, queue, and label manifests and a runtime transition table (Phase 4), plus a pure planner that matches classified artifacts against queues and plans transitions into typed effects and postconditions without touching a backend (Phase 5), plus a runtime executor (`Executor`) that loads fresh Forge state, re-plans, applies label transitions through the `Forge` trait, verifies postconditions, and supports idempotent issue creation via correlation keys (Phase 6). It depends on `harness-forge` for the domain types it classifies and executes against. Later phases (leases, journaling, recovery) are still planned; see ADR 0007.
- `harness-fs` implements filesystem backend support for users, repositories, repository labels, issues, issue comments, pull requests, pull-request comments, pull-request merges, and CI job listing/lookup.
- The next likely implementation task is to continue `harness-workflow` in phases (Phase 7: leases, journaling, and reconciliation), or to add another concrete Forge backend.

## Workspace layout

```text
crates/
  harness-forge/    Domain types and the backend-agnostic Forge interface.
  harness-workflow/ Workflow/orchestration crate (spec, validation, classification, compilation, planning, execution).
  harness-fs/       Local filesystem backend used for fast development and tests.
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
