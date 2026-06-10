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

It includes the hermetic daemon test-worker contract test (`temper-testing`'s
`daemon_worker` test). Cargo builds the `temper-testing` package's
`temper-testing-daemon-worker` test-support binary for that integration test
and exposes its path through `CARGO_BIN_EXE_temper-testing-daemon-worker`.

To prebuild every workspace test harness and integration-test binary without
running tests:

```sh
cargo dev-test-build
```

This expands to:

```sh
cargo test --workspace --all-targets --no-run
```

Use it before the full suite when you want `cargo dev-test-full` to start with
fresh test artifacts already compiled.

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
`forgejo-runner` processes, daemon processes, and worker processes (see
[run-daemon-e2e.md](run-daemon-e2e.md)).
