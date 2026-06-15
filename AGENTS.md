# Agent entry point

This repo follows Diátaxis so that each document has a clear job and agents can
load only the context relevant to their task.

## Rust file organization rules

All LOC figures below are to be intended with blank lines excluded.

- Keep handwritten Rust source files under 300 LOC when practical.
- Files over 400 LOC should usually be split.
- Files over 500 LOC require a short justification in the PR.
- Files over 600 LOC are not allowed unless explicitly allowlisted.
- Exemptions: generated code, large test fixtures, snapshot tests, bindings, and data tables.
- Prefer splitting by domain responsibility, not by arbitrary item type.
- Keep lib.rs, main.rs, and mod.rs as thin facades: declarations, wiring, and re-exports only.
- Keep functions/methods below 75 LOC where practical.

## Formatting

Always run `cargo dev-fmt` before committing changes.

## Useful docs

- [Codebase map](docs/explanation/codebase-map.md)
- [Iterate quickly during local development](docs/how-to/fast-local-iteration.md)
- [End a development session cleanly](docs/how-to/end-a-development-session.md)
