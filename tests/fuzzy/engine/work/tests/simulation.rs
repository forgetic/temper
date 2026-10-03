//! The engine's work hub at random: many random worlds, each settled, every
//! ending reached among them.

use std::collections::BTreeSet;

use temper_engine_domain_work_tests::{ENDINGS, Settings, World};

const ITERATIONS: u32 = 200_000;

fn run(settings: &Settings) -> World {
    let mut world = World::new(*settings);
    world.run(ITERATIONS);
    world
}

#[test]
fn random_worlds_settle_with_every_ending_reached() {
    let mut endings = BTreeSet::new();
    let (mut starts, mut asks, mut applies, mut forgets) = (0, 0, 0, 0);
    for seed in 0..150 {
        let world = run(&Settings::random(seed));
        endings.extend(world.stats().endings.keys().copied());
        let (runs, decisions) = world.judged();
        let (applications, forgotten) = world.judged_answers();
        (starts, asks, applies, forgets) =
            (starts + runs, asks + decisions, applies + applications, forgets + forgotten);
    }
    let missed: Vec<&str> = ENDINGS.iter().copied().filter(|ending| !endings.contains(ending)).collect();
    assert!(missed.is_empty(), "every ending was reached: {missed:?} were not");
    assert!(starts > 1000 && asks > 1000, "the referee judged runs and decisions: {starts}, {asks}");
    assert!(applies > 500 && forgets > 1000, "the referee judged applications and answers: {applies}, {forgets}");
}
