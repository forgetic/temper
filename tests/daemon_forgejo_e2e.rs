//! Daemon-topology live scenarios against real Forgejo.
//!
//! These are the two real-wiring proofs the hermetic `temper-daemon` suite
//! cannot give:
//!
//! 1. **Happy path** — the real `temper-daemon` binary (env→config→composition,
//!    webhook route, role-token routing) plus the deterministic wire-protocol
//!    worker converge one seeded intake issue into a merged, engineer-authored
//!    implementation PR with green real CI and a closed source issue.
//! 2. **CI red→green** — real forgejo-runner CI fails the first head, the
//!    dedicated CI-status poll enqueues exactly one engineer repair, that repair
//!    pushes a marker-bearing green head, and the same targeted CI wake path
//!    lands it before either broad fallback cadence.
//!
//! Each ignored test owns its Forgejo server, host-mode runner, daemon, worker,
//! and scenario repository; drop cleanup kills children on panic.
//!
//! Run with `cargo test --test daemon_forgejo_e2e -- --ignored`
//! (see `docs/how-to/run-daemon-e2e.md`).

#![cfg(unix)]

#[path = "support/daemon_scenario.rs"]
mod daemon_scenario;

use daemon_scenario::{Variant, run_daemon_variant};

// Guards the real daemon binary's wiring: Forgejo API + webhook delivery + git
// auth + role-token PR attribution + mechanical merge-on-green.
#[test]
#[ignore = "boots a real Forgejo + host-mode runner and spawns OS processes; run with --ignored"]
fn daemon_forgejo_happy_path_converges() {
    run_daemon_variant(Variant::happy_path());
}

// Guards real-Actions red→green routing: terminal red must enqueue one engineer
// repair, and terminal green must land through the dedicated CI-status wake.
#[test]
#[ignore = "boots a real Forgejo + host-mode runner and spawns OS processes; run with --ignored"]
fn daemon_forgejo_ci_fails_then_passes_converges() {
    run_daemon_variant(Variant::ci_fails_then_passes());
}
