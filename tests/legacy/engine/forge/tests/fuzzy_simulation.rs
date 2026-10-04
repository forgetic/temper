//! The engine's forge child domain at random: many random worlds, each settled,
//! every ending reached among them.

use std::collections::BTreeSet;

use temper_legacy_engine_forge_world::{ENDINGS, Settings, World};

const ITERATIONS: u32 = 2_000_000;

fn run(settings: &Settings) -> World {
    let mut world = World::new(*settings);
    world.run(ITERATIONS);
    world
}

#[test]
fn random_worlds_settle_with_every_ending_reached() {
    let mut endings = BTreeSet::new();
    let (mut reached, mut writes) = (0, 0);
    for seed in 0..100 {
        let world = run(&Settings::random(seed));
        endings.extend(world.stats().endings.keys().copied());
        let judged = world.judged();
        (reached, writes) = (reached + judged.0, writes + judged.1);
    }
    let missed: Vec<&str> = ENDINGS.iter().copied().filter(|ending| !endings.contains(ending)).collect();
    assert!(missed.is_empty(), "every ending was reached: {missed:?} were not");
    assert!(reached > 1_000 && writes > 1_000, "the referee judged changes and writes: {reached}, {writes}");
}
