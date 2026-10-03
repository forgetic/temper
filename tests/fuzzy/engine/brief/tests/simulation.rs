//! The engine's brief child domain at random: many random worlds, each settled,
//! every ending reached among them.

use std::collections::BTreeSet;

use temper_engine_domain_brief_tests::{ENDINGS, Settings, World};

const ITERATIONS: u32 = 200_000;

fn run(settings: Settings) -> World {
    let mut world = World::new(settings);
    world.run(ITERATIONS);
    world
}

#[test]
fn random_worlds_settle_with_every_ending_reached() {
    let mut endings = BTreeSet::new();
    let (mut sections, mut cuts) = (0, 0);
    for seed in 0..200 {
        let world = run(Settings::random(seed));
        endings.extend(world.stats().endings.keys().copied());
        let judged = world.judged();
        (sections, cuts) = (sections + judged.0, cuts + judged.1);
    }
    let missed: Vec<&str> = ENDINGS.iter().copied().filter(|ending| !endings.contains(ending)).collect();
    assert!(missed.is_empty(), "every ending was reached: {missed:?} were not");
    assert!(sections > 5000 && cuts > 500, "the referee judged sections and cuts: {sections}, {cuts}");
}
