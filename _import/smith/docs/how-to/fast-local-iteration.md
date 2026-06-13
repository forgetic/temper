# Iterate quickly during local development

Smith keeps Cargo aliases for the default local loop.

## Default loop

```sh
cargo dev-check
```

This expands to:

```sh
cargo check --workspace --all-targets
```

Use it while editing APIs and library code.

## Formatting

```sh
cargo fmt --all
```

## Linting

```sh
cargo dev-clippy
```

This expands to `cargo clippy --workspace --all-targets`.

## Tests

For the whole hermetic workspace:

```sh
cargo dev-test
```

Run focused tests while iterating, for example:

```sh
cargo test --workspace --all-targets provider
cargo test --workspace --all-targets product_manager
cargo test --workspace --all-targets workflow_role_decision
```

Live provider and Forgejo checks are ignored/env-gated; use
`run-live-provider-tests.md` before running them.
