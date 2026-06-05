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
- Keep `temper-testing` out of production dependency graphs. It is test support.

## Rust conventions

- Use typed identifiers instead of raw strings in public APIs.
- Prefer explicit state enums over stringly typed statuses.
- Keep async boundaries at the backend interface; concrete backends may use sync
  internals when appropriate.
- Avoid global mutable state.
- Wire new Rust modules into the crate in the same change that creates them;
  `cargo dev-check` must compile all targets, not just production builds.
- Use raw strings for multi-line embedded content such as YAML or shell snippets;
  do not rely on Rust line-continuation string literals to preserve indentation.
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
- Promote durable steering directly into the relevant canonical docs, tests, or
  ADRs instead of adding standalone memory registers.

## Validation before pushing PRs

Run the fast validation loop unless the task explicitly narrows it:

```sh
cargo fmt --all
cargo dev-clippy
cargo dev-check
cargo dev-test-quick
```

If you're touching areas that might break or affect current Forgejo-based
integration tests, or you are adding new Forgejo-based integration tests, run:

``
cargo dev-test-full
```

instead of ```cargo dev-test-quick```.

See:

`docs/how-to/fast-local-iteration.md`,
`docs/how-to/end-a-development-session.md`, and
`docs/how-to/run-forgejo-multiprocess-e2e.md`.
