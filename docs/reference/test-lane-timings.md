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
  - 1,990 tests run; 17 ignored tests now remain skipped (19 in the original
    pre-deletion snapshot).
  - Nextest summary: 10.111 s.
- `cargo dev-test-full`: passed in 33.69 s wall time.
  - Quick lane plus 2 live capstones.
  - Nextest summaries: 7.920 s and 23.644 s.
- `cargo dev-test-e2e-capstones`: passed in 22.50 s wall time.
  - 2 ignored capstones.
  - Nextest summary: 21.584 s.
- `cargo dev-test-e2e-all`: passed in 141.82 s wall time before the redundant
  root `temper run` live scenarios were deleted.
  - 17 ignored/manual live tests remain in the lane.
  - The earlier nextest summary was 140.925 s with 19 live tests.

`cargo dev-test-full` is below the #490 two-minute warmed target on this host.
The all/manual e2e lane is intentionally outside that target.

## Lane membership checked

`cargo nextest list --workspace --run-ignored only -P e2e-capstones` listed:

- `temper::daemon_forgejo_e2e daemon_forgejo_bare_failure_requires_recovery`
- `temper::init_forgejo_e2e init_forgejo_drives_a_working_setup`

`cargo nextest list --workspace --run-ignored only -P e2e` now lists 17 ignored
live tests. The deleted entries are the former root `temper run` implementation
PR handoff and provider server-error retry scenarios; their assertions live in
hermetic real-stack coverage:

- `crates/temper-testing/tests/hermetic_real_stack/basic_delivery.rs::hermetic_real_stack_basic_delivery_architect_triages_then_engineer_opens_pr`
- `crates/temper-testing/tests/hermetic_real_stack.rs::hermetic_real_stack_requeues_provider_server_error_and_later_succeeds`
