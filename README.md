# Harness

Harness is a Rust workspace for building agentic workflows on top of Forge-like collaboration platforms.

The project is intentionally backend-agnostic. The `harness-forge` crate defines the Forge domain model and abstract interface for repositories, users, issues, pull requests, comments, labels, merges, and CI jobs. Concrete backends can adapt that interface to local files, Forgejo, GitHub, or other systems. The placeholder `harness-core` crate is planned to be renamed to `harness-workflow` before workflow/orchestration logic is added.

## Status

This repository is at the design-first scaffold stage. Expect the public interface to evolve, but document every change as if future agents will maintain the project without conversational context.

Current state:

- `harness-forge` contains the provider-neutral Forge domain model and async interface.
- `harness-core` is intentionally minimal; ADR 0007 plans to rename it to `harness-workflow` before adding workflow/orchestration logic.
- `harness-fs` implements filesystem backend support for users, repositories, repository labels, issues, issue comments, pull requests, pull-request comments, pull-request merges, and CI job listing/lookup.
- The next likely implementation task is to implement `harness-workflow` in phases, starting with the crate rename, or to add another concrete Forge backend.

## Workspace layout

```text
crates/
  harness-forge/  Domain types and the backend-agnostic Forge interface.
  harness-core/   Current placeholder for planned harness-workflow crate.
  harness-fs/     Local filesystem backend used for fast development and tests.
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
