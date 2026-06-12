// SPDX-License-Identifier: MPL-2.0

//! Phase 1: multi-worker worlds under seeded schedules and chaos.
//!
//! Each world enqueues a batch of jobs and lets a fleet of simulated workers
//! drain it through the production daemon over real h1 bytes. After the
//! world settles, conservation properties are asserted against the model:
//!
//! - fault-free and delay-chaos worlds: every job applied **exactly once**,
//!   no double dispatch;
//! - cancel-chaos worlds (workers and daemon tasks may die mid-flight):
//!   every job applied **at most once**, and only enqueued jobs ever apply;
//! - any world: the same seed reproduces the identical trace fingerprint.

use std::time::Duration;

use skein::lab::chaos::ChaosConfig;
use temper_sim::scenarios::{cancel_chaos, fleet, run_drain_world};

#[test]
fn fault_free_fleet_applies_every_job_exactly_once() {
    let outcome = run_drain_world(42, None, fleet(4), 12, Duration::from_millis(10));
    outcome.model.assert_exactly_once();
    assert_eq!(outcome.model.applies.len(), 12);
    assert!(
        outcome.model.outstanding.is_empty(),
        "settled world has no outstanding assignments: {:?}",
        outcome.model.outstanding
    );
    // The daemon never shuts down, so its parked drive loop is reported as a
    // "leaked" task at settle time — that one is expected until the daemon
    // grows protocol-grade shutdown. Anything else is a real violation.
    let unexpected: Vec<&String> = outcome
        .invariant_violations
        .iter()
        .filter(|violation| !violation.contains("tasks leaked"))
        .collect();
    assert!(unexpected.is_empty(), "lab invariants: {unexpected:?}");
}

#[test]
fn fault_free_fleet_is_deterministic_per_seed() {
    let a = run_drain_world(42, None, fleet(4), 12, Duration::from_millis(10));
    let b = run_drain_world(42, None, fleet(4), 12, Duration::from_millis(10));
    assert_eq!(a.fingerprint, b.fingerprint);
    assert_eq!(format!("{:?}", a.model), format!("{:?}", b.model));
}

#[test]
fn duplicate_result_submission_applies_once() {
    let mut workers = fleet(2);
    for worker in &mut workers {
        worker.duplicate_results = true;
    }
    let outcome = run_drain_world(7, None, workers, 8, Duration::from_millis(10));
    outcome.model.assert_no_violations();
    // Duplicate submissions must not double-apply: the apply window and
    // grace period absorb them.
    outcome.model.assert_at_most_once();
    assert_eq!(
        outcome.model.applies.len(),
        8,
        "duplicates must not lose jobs either: {:?}",
        outcome.model.applies
    );
}

fn delay_chaos(seed: u64) -> ChaosConfig {
    ChaosConfig {
        seed,
        delay_probability: 0.2,
        delay_range: Duration::ZERO..Duration::from_millis(50),
        wakeup_storm_probability: 0.05,
        wakeup_storm_count: 1..10,
        ..ChaosConfig::off()
    }
}

#[test]
fn delay_chaos_preserves_exactly_once() {
    let outcome = run_drain_world(
        42,
        Some(delay_chaos(42)),
        fleet(4),
        12,
        Duration::from_millis(10),
    );
    outcome.model.assert_exactly_once();
    assert_eq!(outcome.model.applies.len(), 12);
}

#[test]
fn delay_chaos_is_deterministic_per_seed() {
    let a = run_drain_world(
        9,
        Some(delay_chaos(9)),
        fleet(3),
        9,
        Duration::from_millis(10),
    );
    let b = run_drain_world(
        9,
        Some(delay_chaos(9)),
        fleet(3),
        9,
        Duration::from_millis(10),
    );
    assert_eq!(a.fingerprint, b.fingerprint);
    assert_eq!(format!("{:?}", a.model), format!("{:?}", b.model));
}

#[test]
fn cancel_chaos_preserves_at_most_once_across_seeds() {
    for seed in [1u64, 2, 3, 4, 5] {
        let outcome = run_drain_world(
            seed,
            Some(cancel_chaos(seed)),
            fleet(4),
            12,
            Duration::from_millis(10),
        );
        outcome.model.assert_at_most_once();
    }
}

#[test]
fn cancel_chaos_is_deterministic_per_seed() {
    let a = run_drain_world(
        3,
        Some(cancel_chaos(3)),
        fleet(4),
        12,
        Duration::from_millis(10),
    );
    let b = run_drain_world(
        3,
        Some(cancel_chaos(3)),
        fleet(4),
        12,
        Duration::from_millis(10),
    );
    assert_eq!(a.fingerprint, b.fingerprint);
    assert_eq!(format!("{:?}", a.model), format!("{:?}", b.model));
}

#[test]
fn crashing_worker_does_not_lose_the_queue_for_others() {
    let mut workers = fleet(3);
    workers[0].crash_after_assignments = Some(1);
    let outcome = run_drain_world(11, None, workers, 6, Duration::from_millis(10));
    outcome.model.assert_no_violations();
    outcome.model.assert_at_most_once();
    // The crashed worker took one job and never reported it; the others must
    // still drain the rest (the crashed job stays outstanding — lease
    // recovery is a later phase).
    assert!(
        outcome.model.applies.len() >= 5,
        "two healthy workers drain the remaining jobs: {:?}",
        outcome.model.applies
    );
    assert_eq!(outcome.model.outstanding.len(), 1);
}
