# Iterate quickly during local development

Temper defaults are tuned for fast agent iteration rather than production builds.

## Default loop

Use the workspace alias:

```sh
cargo dev-check
```

## Tests

When behavior changes, run relevant tests. For the quick workspace suite:

```sh
cargo dev-test-quick
```

To prebuild every workspace test harness and integration-test binary without
running tests:

```sh
cargo dev-test-build
```

Use it before the full suite when you want `cargo dev-test-full` to start with
fresh test artifacts already compiled.

For the calibrated ignored Forgejo/e2e suite, use the dedicated nextest profile:

```sh
cargo dev-test-e2e
```

This expands to `cargo nextest run --workspace --run-ignored only -P e2e`
with the usual non-interactive output flags. The `e2e` profile caps nextest at
4 test threads so the fixture does not over-schedule Forgejo servers, runners,
daemons, and root-e2e lock waiters on shared developer/CI hosts.

For the full self-contained local suite:

```sh
cargo dev-test-full
```

`cargo dev-test-full` runs the quick suite plus the ignored Forgejo-based
integration tests. It may boot throwaway Forgejo servers, host-mode
`forgejo-runner` processes, daemon processes, and worker processes (see
[run-daemon-e2e.md](run-daemon-e2e.md)).
