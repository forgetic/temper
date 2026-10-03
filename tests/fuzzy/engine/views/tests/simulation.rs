//! The engine's views child domain at random: many random worlds, each settled,
//! every ending reached among them.

use std::collections::BTreeSet;

use temper_engine_domain_views_tests::{ENDINGS, Settings, World};

const ITERATIONS: u32 = 400_000;

fn run(settings: &Settings) -> World {
    let mut world = World::new(*settings);
    world.run(ITERATIONS);
    world
}

#[test]
fn random_worlds_settle_with_every_ending_reached() {
    let mut endings = BTreeSet::new();
    let (mut chunks, mut missed, mut records) = (0, 0, 0);
    for seed in 0..100 {
        let world = run(&Settings::random(seed));
        endings.extend(world.stats().endings.keys().copied());
        let judged = world.judged();
        (chunks, missed, records) = (chunks + judged.0, missed + judged.2, records + judged.3);
    }
    let unreached: Vec<&str> = ENDINGS.iter().copied().filter(|ending| !endings.contains(ending)).collect();
    assert!(unreached.is_empty(), "every ending was reached: {unreached:?} were not");
    assert!(chunks > 5000 && missed > 100 && records > 5000, "the referee judged: {chunks}, {missed}, {records}");
}
