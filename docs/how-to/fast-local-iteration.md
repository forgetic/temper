# Iterate quickly during local development

Harness defaults are tuned for fast agent iteration rather than production builds.

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

When behavior changes, run relevant tests. For the whole workspace:

```sh
cargo dev-test
```

## CPU usage

Cargo uses all available logical CPU cores by default. Keep `.cargo/config.toml` free of a fixed `build.jobs` value unless a task explicitly needs resource limits.

## Profile choices

`Cargo.toml` keeps dev/test profiles optimized for compilation speed:

- no optimization
- reduced debug info
- incremental compilation enabled
- many codegen units
- no LTO

Production profiles are intentionally not tuned yet. Make production-build decisions later when Harness has deployable artifacts and clear release requirements.
