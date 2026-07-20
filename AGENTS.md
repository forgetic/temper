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

## Pre-PR validation

Before pushing or opening an implementation PR, agents must run the repo-local
pre-PR script from the repository root:

```sh
./.temper/pre-pr
```

The script runs these commands in order and stops on the first failure:

1. `cargo fmt --all -- --check`
2. `cargo depgraph-check check`
3. `scripts/check-rust-file-size.sh`
4. `scripts/check-no-ambient-env.sh`
5. Exercise the cached custom-harness permission repair against 0644 fixtures
6. `cargo dev-test-build`
7. Build nextest's exact quick-test binary set, repair custom-harness execute
   bits, then enumerate and run the captured build without invoking Cargo again
8. Drop linked test binaries from `target/debug` before linting
9. `cargo dev-clippy`

Run `cargo dev-scenario-check` or the live `cargo dev-scenario-run` separately
when your change touches scenario manifests, scenario execution, Forgejo/CI
convergence, or post-merge validation evidence.

The full `cargo dev-test-e2e-all` lane is intentionally left to CI (or an
explicit manual local run) so the default pre-PR check stays cheap.

## Useful docs

- [Codebase map](docs/explanation/codebase-map.md)
- [Write Temper tests](docs/how-to/write-temper-tests.md)
- [Iterate quickly during local development](docs/how-to/fast-local-iteration.md)
- [End a development session cleanly](docs/how-to/end-a-development-session.md)
