//! The whole worker in its world: scenarios, replay, and a sweep of random
//! worlds.

use temper_worker_model_tests::{Settings, World};

const ITERATIONS: u32 = 2_000_000;

fn run(settings: &Settings) -> World {
    let mut world = World::new(settings.clone());
    world.run(ITERATIONS);
    world
}

fn count(stats: &temper_worker_model_tests::Stats, kind: &str) -> u32 {
    stats.answers.get(kind).copied().unwrap_or(0)
}

#[test]
fn a_calm_world_answers_every_run_and_settles() {
    let world = run(&Settings::calm(1));
    let stats = world.stats();
    let tally = world.tally();
    assert_eq!(tally.assigned, stats.answers_sent, "every assignment is answered: {stats:?}");
    assert_eq!(stats.answers_lost + stats.held + stats.drops, 0, "the channel never drops: {stats:?}");
    assert!(count(&stats, "ended") > 0 && count(&stats, "parked") > 0, "{stats:?}");
    assert!(stats.landed > 0 && stats.edits > 0, "runs push what they edited: {stats:?}");
    assert!(stats.relayed > 0 && stats.events > 0, "the engine answers relays and sends events: {stats:?}");
}

#[test]
fn a_seed_replays_to_the_same_run() {
    let trace = temper_world::assert_replays(11, 12, |seed| {
        let world = run(&Settings::calm(seed));
        (world.trace().to_vec(), (world.stats(), world.tally(), world.now()))
    });
    assert!(trace.len() > 100, "the world did something");
}
