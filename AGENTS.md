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
- `harness-core` is reserved for workflow and orchestration logic to be defined later.
- `harness-fs` implements local filesystem backend support for users, repositories, repository labels, issues, issue comments, pull requests, pull-request comments, and pull-request merges; remaining Forge operations intentionally return portable unsupported-operation errors.
- Documentation follows Diátaxis and is part of the product, not an afterthought.
- Agent lessons live in `docs/reference/agent-lessons/` so corrections survive across sessions.
- The next likely work is extending the filesystem backend with CI job support, refining the interface only as backend constraints appear.

## Ground rules

- Keep the Forge interface backend-agnostic. Do not leak Forgejo, GitHub, or filesystem-specific concepts into `harness-forge` unless they are modeled as portable concepts.
- Prefer small, documented changes. Future agents should be able to infer intent from committed files alone.
- Every public Forge API change must update `docs/reference/forge-interface.md`.
- Every domain-model change must update `docs/explanation/domain-model.md` when it changes the conceptual model.
- Add or update tests with behavior changes.
- Keep Rust source and test files at or below 600 lines; split focused modules or shared test support before exceeding that budget.
- Run `cargo fmt --all` and `cargo dev-check` before handing off.

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
- Keep `harness-core` focused on workflow and orchestration logic.
- Use typed identifiers instead of raw strings in public APIs.
- Prefer explicit state enums over stringly-typed statuses.
- Keep async boundaries at the backend interface; concrete backends may use sync internals if appropriate.
- Avoid global mutable state.

## Fast local iteration

- Use `cargo dev-check` for the default validation loop; it checks the whole workspace and all targets without producing binaries.
- Cargo uses all available logical CPU cores by default. Do not set a fixed `build.jobs` value unless the task explicitly requires it.
- Development and test profiles are tuned for fast local compilation in `Cargo.toml`; production profiles can be designed later.

## Backend conventions

Each backend should document:

- Supported operations.
- Persistence model and schema, if any.
- Consistency guarantees.
- Unsupported Forge features and how they fail.

The filesystem backend is the reference backend for deterministic tests and fast iteration, not a production forge.

## Session closeout

Before handing off, follow `docs/how-to/end-a-development-session.md`. Start the review from top-level `README.md` and `AGENTS.md`, then update any task-relevant docs so the next agent can continue without hidden context. If the session involved a correction, failed assumption, or missing guidance, record or update an agent lesson.
