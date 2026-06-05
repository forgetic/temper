# Iterate quickly during local development

Temper defaults are tuned for fast agent iteration rather than production builds.

## Default loop

Use the workspace alias:

```sh
cargo dev-check
```

This expands to:

```sh
cargo check --workspace --all-targets
```

`cargo check` validates types and borrow checking without producing final binaries, so it should be the default command while designing APIs and editing library code.

## Linting

Clippy is installed in this environment. Run it before handoff:

```sh
cargo dev-clippy
```

This expands to `cargo clippy --workspace --all-targets`. Keep the lint output clean.

## Formatting

Run formatting before handoff:

```sh
cargo fmt --all
```

## Tests

When behavior changes, run relevant tests. For the whole default workspace suite:

```sh
cargo dev-test
```

`cargo dev-test` includes the fast filesystem multi-process rehearsals
(`temper-testing`'s `multiprocess` and `multi_repo_multiprocess` tests). Cargo
builds the `temper-testing` package's `temper-testing-worker` test-support
binary for those integration tests and exposes its path through
`CARGO_BIN_EXE_temper-testing-worker`; no nested Cargo build runs inside the
default suite. Set `TEMPER_TESTING_WORKER_BIN` only when you intentionally want
to spawn a prebuilt worker binary. Keep the whole default suite fast; as a soft
target for agent changes, it should complete in under about 10 seconds on a
warmed local checkout. If a change makes the default suite slower, prefer moving
slow coverage behind `#[ignore]` and document how to run it before handoff.

## CPU usage

Cargo uses all available logical CPU cores by default. Keep `.cargo/config.toml` free of a fixed `build.jobs` value unless a task explicitly needs resource limits.

## Profile choices

`Cargo.toml` keeps dev/test profiles optimized for compilation speed:

- no optimization
- reduced debug info
- incremental compilation enabled
- many codegen units
- no LTO

Production profiles are intentionally not tuned yet. Make production-build decisions later when Temper has deployable artifacts and clear release requirements.
