# Development conventions

This page holds stable contribution rules for Temper. Use it with the
session-start and session-closeout how-to guides.

## General rules

- Prefer small, documented changes. Future agents should be able to infer intent
  from committed files alone.
- Add or update tests for behavior changes.
- Every public Forge API change must update `docs/reference/forge-interface.md`.
- Domain-model changes must update `docs/explanation/domain-model.md` when they
  change the conceptual model.

## Crate boundaries

- Keep the Forge interface backend-agnostic. Do not leak Forgejo, GitHub, or
  filesystem-specific concepts into `temper-forge` unless they are modeled as
  portable concepts.
- Keep `temper-forge` free of concrete backend dependencies.
- Keep `temper-workflow` focused on workflow and orchestration logic.
- Keep LLM SDK usage inside `temper-agents`; workflow state mutations still go
  through runner tools such as `RoleTools`.
- Keep `temper-testing` out of production dependency graphs. It is test support
  and may be a dev-dependency or a dependency of test-only crates.

## Rust conventions

- Use typed identifiers instead of raw strings in public APIs.
- Prefer explicit state enums over stringly typed statuses.
- Keep async boundaries at the backend interface; concrete backends may use sync
  internals when appropriate.
- Avoid global mutable state.
- Keep Rust source and test files at or below 600 lines; split focused modules or
  shared test support before exceeding that budget.

## Backend conventions

Each backend should document:

- supported operations;
- persistence model and schema, if any;
- consistency guarantees;
- unsupported Forge features and how they fail.

The filesystem and in-memory backends are the reference backends for deterministic
iteration. They must keep the same observable contract; a behavior change to one
usually needs the same change in the other, plus updates to both backend
reference pages.

## Documentation conventions

- Follow Diátaxis: tutorials teach, how-to guides solve tasks, reference pages
  define contracts, explanation pages give rationale, and ADRs record decisions.
- Keep hand-written docs focused: aim for about 150 lines or fewer and split
  before about 350 lines; prefer short index pages over catch-all documents.
- Do not mix tutorial prose into reference pages or hide contracts in explanation
  pages.
- Capture recurring mistakes or human steering in `docs/reference/agent-lessons/`
  and promote durable rules to the canonical docs.

## Validation before handoff

Run the fast validation loop unless the task explicitly narrows it:

```sh
cargo fmt --all
cargo dev-clippy
cargo dev-check
```

Run task-specific tests when behavior changed. See
`docs/how-to/fast-local-iteration.md` and
`docs/how-to/end-a-development-session.md`.
