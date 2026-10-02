//! The work hub in its world: scenarios, replay, and a sweep of random worlds.

use std::collections::BTreeSet;

use temper_engine_model_work::Limits;
use temper_engine_model_work_tests::{ENDINGS, LIMITS, Settings, Span, Stats, World};
use temper_world::assert_replays;

const ITERATIONS: u32 = 200_000;

fn run(settings: &Settings) -> World {
    let mut world = World::new(*settings);
    world.run(ITERATIONS);
    world
}

fn count(stats: &Stats, ending: &str) -> u32 {
    stats.endings.get(ending).copied().unwrap_or(0)
}

#[test]
fn a_calm_world_does_every_item() {
    for seed in 0..5 {
        let world = run(&Settings::calm(seed));
        let stats = world.stats();
        assert_eq!(world.ended(), (stats.items, 0), "seed {seed}: every item done: {stats:?}");
        assert!(count(&stats, "parked") > 0 && count(&stats, "acted") > 0, "seed {seed}: {stats:?}");
        assert!(stats.relayed > 0, "seed {seed}: messages reach runs: {stats:?}");
        let (runs, decisions) = world.judged();
        assert!(runs >= u64::from(stats.items) && decisions > runs, "the referee judged: {runs}, {decisions}");
    }
}

#[test]
fn a_working_set_smaller_than_the_work_takes_items_in_as_others_end() {
    let settings = Settings {
        limits: Limits { items: 2, ..LIMITS },
        items: 10,
        hand_in_gap: Span::millis(1, 10),
        ..Settings::calm(2)
    };
    let world = run(&settings);
    let stats = world.stats();
    assert!(count(&stats, "full") > 0, "{stats:?}");
    assert_eq!(world.ended(), (10, 0), "nothing taken in is dropped: {stats:?}");
}

#[test]
fn runs_that_fail_are_retried_and_then_held_and_people_release_them() {
    let settings = Settings { fails: 600, releases: 1000, ..Settings::calm(3) };
    let world = run(&settings);
    let stats = world.stats();
    assert!(count(&stats, "held: failures") > 0 && count(&stats, "released") > 0, "{stats:?}");
    let (done, held) = world.ended();
    assert_eq!(done + held, stats.items, "{stats:?}");
}

#[test]
fn outcomes_stale_invalid_or_waiting_for_acceptance_are_judged_again_once_accepted() {
    let settings = Settings { stale: 150, invalid: 150, accepting: 300, releases: 1000, ..Settings::calm(4) };
    let world = run(&settings);
    let stats = world.stats();
    for ending in ["stale", "invalid", "held: acceptance", "released"] {
        assert!(count(&stats, ending) > 0, "{ending}: {stats:?}");
    }
}

#[test]
fn people_stop_runs_and_the_items_are_held() {
    let settings = Settings { stops: 6, ..Settings::calm(5) };
    let world = run(&settings);
    let stats = world.stats();
    assert!(stats.stops > 0 && count(&stats, "held: stopped") > 0, "{stats:?}");
}

#[test]
fn workers_that_drop_come_back_with_their_runs_or_are_presumed_lost() {
    let settings = Settings { drops: 400, returns: 500, ..Settings::calm(6) };
    let world = run(&settings);
    let stats = world.stats();
    assert!(count(&stats, "lost") > 0, "{stats:?}");
    let (done, held) = world.ended();
    assert_eq!(done + held, stats.items, "{stats:?}");
}

#[test]
fn a_forge_that_fails_and_answers_late_still_ends_every_item() {
    let settings = Settings {
        transient: 300,
        refused: 40,
        late: 300,
        lateness: Span::millis(0, 5000),
        broken: 50,
        releases: 1000,
        ..Settings::calm(7)
    };
    let world = run(&settings);
    let stats = world.stats();
    assert!(stats.transients > 0 && stats.refusals > 0 && stats.lates > 0, "{stats:?}");
    assert!(count(&stats, "held: record") + count(&stats, "held: writes") > 0, "{stats:?}");
}

#[test]
fn the_engine_restarts_and_rebuilds_the_hub_from_the_records() {
    let mut adopted = 0;
    let mut resumed = 0;
    for seed in 0..20 {
        let settings = Settings { restarts: 3, restart_gap: Span::millis(2000, 15_000), ..Settings::calm(seed) };
        let world = run(&settings);
        let stats = world.stats();
        assert_eq!(stats.restarts, 3, "{stats:?}");
        let (done, held) = world.ended();
        assert_eq!(done + held, stats.items, "seed {seed}: {stats:?}");
        adopted += count(&stats, "adopted");
        resumed += count(&stats, "resumed");
    }
    assert!(adopted > 0 && resumed > 0, "restarts adopt runs and resume applications: {adopted}, {resumed}");
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
    let (mut starts, mut asks) = (0, 0);
    for seed in 0..150 {
        let world = run(&Settings::random(seed));
        endings.extend(world.stats().endings.keys().copied());
        let judged = world.judged();
        (starts, asks) = (starts + judged.0, asks + judged.1);
    }
    let missed: Vec<&str> = ENDINGS.iter().copied().filter(|ending| !endings.contains(ending)).collect();
    assert!(missed.is_empty(), "every ending was reached: {missed:?} were not");
    assert!(starts > 1000 && asks > 1000, "the referee judged runs and decisions: {starts}, {asks}");
}
