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

## Tests

When behavior changes, run relevant tests. For the quick workspace suite:

```sh
cargo dev-test-quick
```

This expands to:

```sh
cargo test --workspace --all-targets
```

`cargo dev-test-quick` runs non-ignored tests.

It includes the fast filesystem multi-process rehearsals (`temper-testing`'s
`multiprocess` and `multi_repo_multiprocess` tests). Cargo builds the
`temper-testing` package's `temper-testing-worker` test-support binary for those
integration tests and exposes its path through
`CARGO_BIN_EXE_temper-testing-worker`. Set `TEMPER_TESTING_WORKER_BIN` only when
you intentionally want to spawn a prebuilt worker binary.

For the full self-contained local suite:

```sh
cargo dev-test-full
```

This expands to:

```sh
cargo test --workspace --all-targets -- --include-ignored
```

`cargo dev-test-full` runs the quick suite plus the ignored Forgejo-based
integration tests. It may boot throwaway Forgejo servers, host-mode
`forgejo-runner` processes, local webhook triggers, and worker processes.
