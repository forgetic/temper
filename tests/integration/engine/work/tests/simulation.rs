//! The work hub in its world: scenarios, replay, and a sweep of random worlds.

use std::collections::BTreeSet;

use temper_engine_model_work::Limits;
use temper_engine_model_work_tests::{LIMITS, Settings, Span, Stats, World};
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
    let mut endings = BTreeSet::new();
    for seed in 0..5 {
        let settings = Settings { stale: 150, invalid: 150, accepting: 300, releases: 1000, ..Settings::calm(seed) };
        let world = run(&settings);
        endings.extend(world.stats().endings.keys().copied());
        let (applies, _) = world.judged_answers();
        assert!(applies > 0, "seed {seed}: the referee judged applications");
    }
    for ending in ["stale", "invalid", "held: acceptance", "released"] {
        assert!(endings.contains(ending), "{ending}: {endings:?}");
    }
}

#[test]
fn a_fleet_that_refuses_and_hands_back_undelivered_events_counts_no_failure() {
    let settings = Settings { refusals: 300, drops: 300, returns: 1000, messages: 60, ..Settings::calm(8) };
    let world = run(&settings);
    let stats = world.stats();
    assert!(count(&stats, "refused") > 0 && count(&stats, "undelivered") > 0, "{stats:?}");
    assert_eq!(count(&stats, "held: failures"), 0, "a refusal is no failure: {stats:?}");
    assert_eq!(world.ended(), (stats.items, 0), "{stats:?}");
}

#[test]
fn people_stop_runs_whose_claims_are_being_written() {
    let mut stopped = 0;
    for seed in 0..10 {
        let settings = Settings { stops: 20, latency: Span::millis(200, 2000), ..Settings::calm(seed) };
        let stats = run(&settings).stats();
        stopped += count(&stats, "stopped claiming");
    }
    assert!(stopped > 0, "a stop came while a claim was written");
}

#[test]
fn a_mangled_record_holds_its_item_until_a_person_mends_it() {
    let mut mangled = 0;
    let mut listed = 0;
    for seed in 0..20 {
        let settings = Settings {
            mangles: 2,
            restarts: 2,
            restart_gap: Span::millis(5000, 15_000),
            drops: 0,
            ..Settings::calm(seed)
        };
        let world = run(&settings);
        let stats = world.stats();
        mangled += count(&stats, "mangled");
        listed += count(&stats, "listed");
        let (done, held) = world.ended();
        assert_eq!(done + held, stats.items, "seed {seed}: {stats:?}");
    }
    assert!(mangled > 0 && listed > 0, "records were read mangled, and strays listed: {mangled}, {listed}");
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
