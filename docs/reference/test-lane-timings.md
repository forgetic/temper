# Test lane timings

This page records the timing snapshot used to close the #490 test-layer split.
It is evidence for lane shape, not a guaranteed SLA: live Forgejo timings vary
with host CPU, cache warmth, and runner load.

## 2026-06-28 warmed snapshot

Environment: writable agent checkout on Linux x86_64, cargo-nextest 0.9.137,
`TEMPER_TEST_CONVERGENCE_TIMEOUT_SECS=300`. The host's global cargo config
pointed at a missing `kache` wrapper, so the measured commands used
`RUSTC_WRAPPER=`. Cargo test artifacts and the Forgejo fixture cache were warm
before the rows below were taken. Test counts here are nextest-discovered run
counts; the source inventory is a separate static snapshot.

- `cargo dev-test-quick`: passed in 11.32 s wall time.
  - 1,990 tests run; 19 ignored tests skipped.
  - Nextest summary: 10.111 s.
- `cargo dev-test-full`: passed in 33.69 s wall time.
  - Quick lane plus 2 live capstones.
  - Nextest summaries: 7.920 s and 23.644 s.
- `cargo dev-test-e2e-capstones`: passed in 22.50 s wall time.
  - 2 ignored capstones.
  - Nextest summary: 21.584 s.
- `cargo dev-test-e2e-all`: passed in 141.82 s wall time.
  - 19 ignored/manual live tests.
  - Nextest summary: 140.925 s.

`cargo dev-test-full` is below the #490 two-minute warmed target on this host.
The all/manual e2e lane is intentionally outside that target.

## Lane membership checked

`cargo nextest list --workspace --run-ignored only -P e2e-capstones` listed:

- `temper::daemon_forgejo_e2e daemon_forgejo_ci_fails_then_passes_converges`
- `temper::init_forgejo_e2e init_forgejo_drives_a_working_setup`

`cargo nextest list --workspace --run-ignored only -P e2e` listed 19 ignored
live tests, including both `tests/run_forgejo_e2e.rs` scenarios.

The checkpointed standalone `temper run` PR-handoff scenario stays in the
manual/all-e2e lane. It was safe to remove from the default capstone list
because `crates/temper-testing/tests/hermetic_real_stack/checkpoint_pr.rs`
proves the checkpoint→PR handoff with the real worker/daemon/native-agent stack
over in-process transport.
