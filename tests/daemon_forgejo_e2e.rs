//! Daemon-topology live scenarios against real Forgejo.
//!
//! These are the two real-wiring proofs the hermetic `temper-daemon` suite
//! cannot give:
//!
//! 1. **Happy path** — the real `temper-daemon` binary (env→config→composition,
//!    webhook route, role-token routing) plus the deterministic wire-protocol
//!    worker converge one seeded intake issue into a merged, engineer-authored
//!    implementation PR with green real CI and a closed source issue.
//! 2. **Ambiguous CI failure** — real Forgejo 16.0.1 Actions returns a
//!    status-only terminal failure through the provider-run jobs API; the
//!    dedicated CI poll keeps the gate red without dispatching writable repair
//!    or advancing the PR head.
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

// Guards real Actions ambiguous-failure routing: a terminal status-only failure
// must not enqueue an engineer repair, synthesize a new head, or satisfy landing.
#[test]
#[ignore = "boots a real Forgejo + host-mode runner and spawns OS processes; run with --ignored"]
fn daemon_forgejo_bare_failure_requires_recovery() {
    run_daemon_variant(Variant::ambiguous_ci_failure());
}
