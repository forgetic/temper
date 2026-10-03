//! The engine's world at random: many random worlds, each settled, every
//! ending reached among them; and the seeds that once found what the engine
//! did not do, kept now that it does.

use std::collections::BTreeSet;

use temper_engine_model_tests::{ENDINGS, Settings, World};

const ITERATIONS: u32 = 200_000;

fn run(settings: Settings) -> World {
    let mut world = World::new(settings);
    world.run(ITERATIONS);
    world
}

/// Seeds that found what the engine did not do: a change approved on its
/// head that it never read again (60); items claimed whose runs were never
/// assigned once a read of their briefs failed (248, 267).
const FINDINGS: [u64; 3] = [60, 248, 267];

#[test]
fn random_worlds_settle_with_every_ending_reached() {
    let mut endings = BTreeSet::new();
    let mut judged = (0, 0);
    for seed in 0..100 {
        let world = run(Settings::random(seed));
        endings.extend(world.stats().endings.keys().copied());
        let (checks, met) = world.judged();
        judged = (judged.0 + checks, judged.1 + met);
    }
    let missed: Vec<&str> = ENDINGS.iter().copied().filter(|ending| !endings.contains(ending)).collect();
    assert!(missed.is_empty(), "every ending was reached: {missed:?} were not");
    assert!(judged.0 > 5_000 && judged.1 > 300, "the referee judged: {judged:?}");
}

/// Random worlds whose stories include plans', over fewer seeds: a plan's
/// world is several times longer.
#[test]
fn random_worlds_with_plans_settle() {
    for seed in 0..12 {
        run(Settings::planning(seed));
    }
}

/// The engine restarting at drawn moments: it rebuilds from the forge and
/// the store what it decides on, and makes nothing twice.
#[test]
fn restarting_worlds_settle() {
    for seed in 0..40 {
        run(Settings::restarting(seed));
    }
}

#[test]
fn the_engine_findings_replay() {
    for seed in FINDINGS {
        run(Settings::random(seed));
    }
}
