// SPDX-License-Identifier: MPL-2.0

//! Phase 2: the seed corpus.
//!
//! Two layers of coverage, both fully deterministic per seed:
//!
//! - `regression_seed_corpus_holds_invariants` replays every seed that has
//!   ever mattered (including any that once exposed a bug — add failing
//!   seeds here permanently, they reproduce forever);
//! - `fresh_seed_batch_holds_invariants` sweeps a batch of new seeds each CI
//!   run when `TEMPER_SIM_SEED_BASE` is exported (e.g. the CI run number);
//!   without it, a fixed default base keeps the test meaningful locally.
//!
//! A failure always prints the offending seed: rerun with that seed in the
//! regression corpus to reproduce exactly.

use std::time::Duration;

use temper_sim::scenarios::{cancel_chaos, fleet, run_drain_world};

/// Seeds pinned forever. Grow this list with every seed that ever caught a
/// bug — replaying them is free.
const REGRESSION_SEEDS: &[u64] = &[1, 2, 3, 4, 5, 7, 9, 11, 42, 1337];

fn assert_world_invariants(seed: u64) {
    let outcome = run_drain_world(
        seed,
        Some(cancel_chaos(seed)),
        fleet(4),
        12,
        Duration::from_millis(10),
    );
    // At-most-once must survive arbitrary cancellation chaos; double
    // applies under ANY schedule are bugs.
    outcome.model.assert_at_most_once();
    let unexpected: Vec<&String> = outcome
        .invariant_violations
        .iter()
        .filter(|violation| !violation.contains("tasks leaked"))
        .collect();
    assert!(
        unexpected.is_empty(),
        "seed {seed}: lab invariants violated: {unexpected:?}"
    );
}

#[test]
fn regression_seed_corpus_holds_invariants() {
    for &seed in REGRESSION_SEEDS {
        eprintln!("temper-sim: regression seed {seed}");
        assert_world_invariants(seed);
    }
}

#[test]
fn fresh_seed_batch_holds_invariants() {
    let base: u64 = std::env::var("TEMPER_SIM_SEED_BASE")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0x00C0_FFEE);
    for offset in 0..10u64 {
        let seed = base.wrapping_mul(1_000_003).wrapping_add(offset);
        eprintln!("temper-sim: fresh seed {seed} (base {base})");
        assert_world_invariants(seed);
    }
}
