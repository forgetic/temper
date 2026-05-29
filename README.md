# Harness

Harness is a Rust workspace for building agentic workflows on top of Forge-like collaboration platforms.

The project is intentionally backend-agnostic. The `harness-forge` crate defines the Forge domain model and abstract interface for repositories, users, issues, pull requests, comments, labels, merges, and CI jobs. Concrete backends can adapt that interface to local files, Forgejo, GitHub, or other systems. The `harness-core` crate is reserved for workflow and orchestration logic.

## Status

This repository is at the design-first scaffold stage. Expect the public interface to evolve, but document every change as if future agents will maintain the project without conversational context.

Current state:

- `harness-forge` contains the provider-neutral Forge domain model and async interface.
- `harness-core` is intentionally minimal and reserved for workflow/orchestration logic.
- `harness-fs` implements filesystem backend support for users, repositories, repository labels, issues, issue comments, pull requests, pull-request comments, and pull-request merges; CI jobs intentionally return portable unsupported-operation errors.
- The next likely implementation task is to extend the filesystem backend with CI job support.

## Workspace layout

```text
crates/
  harness-forge/  Domain types and the backend-agnostic Forge interface.
  harness-core/   Workflow and orchestration logic.
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
