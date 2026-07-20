# Iterate quickly during local development

Temper defaults are tuned for fast agent iteration rather than production builds.

## Default loop

Use the workspace alias:

```sh
cargo dev-check
```

Validate checked-in scenario manifests and their local fixture references with
the same cheap check that runs in fast CI:

```sh
cargo dev-scenario-check
```

Exercise the validation-grade live `basic-delivery` lane. This builds a
standalone `temper` binary, then runs real Forgejo + real `forgejo-runner` CI +
real standalone Temper + Jig fake LLM agents through the live-only
`runner.uses = "manifest"` path:

```sh
cargo dev-scenario-run
```

The manifest runner intentionally rejects hermetic, MemoryForge-only, and
in-process Temper substitutes. For fast local coverage, use focused crate tests
or `cargo dev-test-quick`; do not cite those lower-confidence checks as
validation-grade scenario evidence.

## Tests

When behavior changes, run relevant tests. For the quick workspace suite:

```sh
cargo dev-test-quick
```

The required pre-PR lane uses `scripts/run-nextest-quick.sh` instead. That
script captures nextest's final binaries-only build, repairs execute bits on
cached custom harnesses, and then enumerates and runs the captured build without
letting Cargo restore those artifacts again.

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

This runs only the two ignored capstone tests named in the `e2e-capstones`
nextest profile: daemon CI red→green convergence and `temper init --apply`.
The shorter `cargo dev-test-e2e` shorthand points at this same capstone lane.
The former root `temper run` live scenarios were deleted because `temper-testing`
now covers the implementation-PR handoff and provider server-error retry/requeue
paths with faster hermetic real-stack tests.

For every ignored/manual live test, including lower-level Forgejo fixture
smokes, provisioning checks, provider/OAuth self-skipping probes, and the root
Forgejo scenarios outside the capstone list, use:

```sh
cargo dev-test-e2e-all
```

This expands to `cargo nextest run --workspace --run-ignored only -P e2e`
with the usual non-interactive output flags. The `e2e` profile caps nextest at
4 test threads and assigns root Forgejo e2es to the `root-forgejo-e2e` test
group (`max-threads = 1`) so the scheduler queues those heavyweight process
trees instead of letting them start in parallel and block on their advisory
lock.

For the full self-contained local suite:

```sh
cargo dev-test-full
```

`cargo dev-test-full` runs `cargo dev-test-quick --no-fail-fast` and then
`cargo dev-test-e2e-capstones`. It deliberately does **not** use
`--run-ignored all`: excluded live scenarios are still present in
`cargo dev-test-e2e-all` until their assertions are either promoted to the
capstone list or covered by hermetic real-stack tests (see
[run-daemon-e2e.md](run-daemon-e2e.md)).
