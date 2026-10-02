//! The forge sub-model in its world: scenarios, replay, and a sweep of random
//! worlds.

use std::collections::BTreeSet;

use temper_engine_model_forge::Limits;
use temper_engine_model_forge_tests::{ENDINGS, Settings, Stats, World, people};
use temper_forge_model::Config;
use temper_world::assert_replays;

const ITERATIONS: u32 = 2_000_000;

fn run(settings: &Settings) -> World {
    let mut world = World::new(*settings);
    world.run(ITERATIONS);
    world
}

fn count(stats: &Stats, ending: &str) -> u32 {
    stats.endings.get(ending).copied().unwrap_or(0)
}

#[test]
fn a_calm_world_keeps_up_and_settles() {
    let world = run(&Settings::calm(1));
    let stats = world.stats();
    for ending in [
        "announced: missing",
        "offered",
        "news: comment",
        "news: pull",
        "changed",
        "left",
        "loaded",
        "wrote",
        "wrote: created",
        "wrote: commented",
    ] {
        assert!(count(&stats, ending) > 0, "{ending}: {stats:?}");
    }
    for ending in ["timed out", "limited", "full", "wrote: failed"] {
        assert_eq!(count(&stats, ending), 0, "{ending}: {stats:?}");
    }
    let (reached, writes) = world.judged();
    assert!(reached > 10 && writes > 10, "the referee judged: {reached}, {writes}");
}

#[test]
fn every_change_reaches_the_working_set_with_every_webhook_lost() {
    for seed in 0..10 {
        let calm = Settings::calm(seed);
        let settings = Settings { forge: Config { hooks_lost: 1000, ..calm.forge }, ..calm };
        let world = run(&settings);
        let stats = world.stats();
        let forge = stats.forge.expect("counted");
        assert_eq!(forge.hooks, 0, "seed {seed}: no webhook arrived: {forge:?}");
        assert!(world.judged().0 > 0, "seed {seed}: changes reached the working set: {stats:?}");
    }
}

#[test]
fn a_rate_limit_refusal_holds_every_call_until_its_reset() {
    // The engine's own budget is set above the forge's limit, so the forge
    // refuses, and the engine waits for each reset.
    let calm = Settings::calm(3);
    let settings =
        Settings { limits: Limits { rate: 80, ..calm.limits }, forge: Config { rate_limit: 50, ..calm.forge }, ..calm };
    let stats = run(&settings).stats();
    assert!(count(&stats, "limited") > 0, "{stats:?}");
    assert!(stats.forge.expect("counted").limited > 0, "{stats:?}");
}

#[test]
fn restarts_start_cold_and_make_nothing_twice() {
    for seed in 0..10 {
        let calm = Settings::calm(seed);
        let settings = Settings {
            forge: Config { timeouts: 100, ..calm.forge },
            restarts: 4,
            restart_at: temper_engine_model_forge_tests::Span::millis(30_000, 500_000),
            ..calm
        };
        let stats = run(&settings).stats();
        assert_eq!(stats.restarts, 4, "seed {seed}: {stats:?}");
        // A restart may interrupt a cold start; the referee holds each one
        // not interrupted to its bound.
        assert!(count(&stats, "loaded") >= 3, "seed {seed}: a cold start per life: {stats:?}");
        assert!(count(&stats, "announced: found") > 0, "seed {seed}: records found again: {stats:?}");
    }
}

#[test]
fn creations_whose_answers_were_lost_are_found_by_their_keys() {
    let mut found = 0;
    for seed in 0..10 {
        let calm = Settings::calm(seed);
        let settings = Settings {
            forge: Config { timeouts: 200, late: 100, ..calm.forge },
            parent: temper_engine_model_forge_tests::parent::Script { tasks: 500, replies: 600, ..calm.parent },
            ..calm
        };
        let stats = run(&settings).stats();
        assert!(count(&stats, "timed out") > 0, "seed {seed}: {stats:?}");
        found += count(&stats, "found");
    }
    assert!(found > 0, "creations were found by their keys");
}

#[test]
fn a_full_working_set_refuses_new_work_which_waits_on_the_forge() {
    let calm = Settings::calm(4);
    let settings = Settings {
        limits: Limits { items: 3, ..calm.limits },
        parent: temper_engine_model_forge_tests::parent::Script { closes: 300, ..calm.parent },
        ..calm
    };
    let stats = run(&settings).stats();
    assert!(count(&stats, "full") > 0, "{stats:?}");
    assert!(stats.peak <= 3, "{stats:?}");
}

#[test]
fn a_record_a_person_mangled_holds_its_item_until_released() {
    let mut held = 0;
    for seed in 0..10 {
        let calm = Settings::calm(seed);
        let weights = people::Weights { mangles: 6, ..calm.people.weights };
        let settings = Settings { people: people::Script { weights, ..calm.people }, restarts: 1, ..calm };
        let stats = run(&settings).stats();
        held += stats.parent.holds;
    }
    assert!(held > 0, "items were held for a person");
}

#[test]
fn a_seed_replays_to_the_same_run() {
    let run = |seed: u64| {
        let world = run(&Settings::random(seed));
        (world.trace().to_vec(), (world.stats(), world.now()))
    };
    let trace = assert_replays(7, 8, run);
    assert!(trace.len() > 100, "the world did something");
}

#[test]
fn facts_change_nothing() {
    for seed in 0..5 {
        let settings = Settings::random(seed);
        let none = run(&Settings { limits: Limits { facts: 0, ..settings.limits }, ..settings });
        let many = run(&Settings { limits: Limits { facts: 4096, ..settings.limits }, ..settings });
        assert!(none.trace() == many.trace(), "seed {seed}: the same run whatever facts are kept");
    }
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
