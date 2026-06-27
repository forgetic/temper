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

For the default live Forgejo capstones, use:

```sh
cargo dev-test-e2e-capstones
```

This runs only the three ignored capstone tests named in the
`e2e-capstones` nextest profile: daemon CI red→green convergence,
`temper init --apply`, and the checkpointed `temper run` fake-LLM PR handoff.
The shorter `cargo dev-test-e2e` shorthand points at this same capstone lane.

For every ignored/manual live test, including lower-level Forgejo fixture
smokes, provisioning checks, provider/OAuth self-skipping probes, and the root
Forgejo scenarios outside the capstone list, use:

```sh
cargo dev-test-e2e-all
```

This expands to `cargo nextest run --workspace --run-ignored only -P e2e`
with the usual non-interactive output flags. The `e2e` profile caps nextest at
4 test threads so the fixture does not over-schedule Forgejo servers, runners,
daemons, and root-e2e lock waiters on shared developer/CI hosts.

For the full self-contained local suite:

```sh
cargo dev-test-full
```

`cargo dev-test-full` runs `cargo dev-test-quick --no-fail-fast` and then
`cargo dev-test-e2e-capstones`. It deliberately does **not** use
`--run-ignored all`: excluded live scenarios are still present in
`cargo dev-test-e2e-all` until their assertions are either promoted to the
capstone list or covered by future hermetic real-stack tests (see
[run-daemon-e2e.md](run-daemon-e2e.md)).
