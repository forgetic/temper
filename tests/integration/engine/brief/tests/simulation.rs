//! The brief in its world: scenarios, replay, and a sweep of random worlds.

use std::collections::BTreeSet;

use temper_engine_model_brief::Limits;
use temper_engine_model_brief_tests::{ENDINGS, LIMITS, Settings, Span, Stats, World};
use temper_world::assert_replays;

const ITERATIONS: u32 = 200_000;

fn run(settings: Settings) -> World {
    let mut world = World::new(settings);
    world.run(ITERATIONS);
    world
}

fn count(stats: &Stats, ending: &str) -> u32 {
    stats.endings.get(ending).copied().unwrap_or(0)
}

#[test]
fn a_calm_world_renders_every_brief_and_settles() {
    let world = run(Settings::calm(1));
    let stats = world.stats();
    assert_eq!(count(&stats, "rendered"), stats.briefs, "every brief rendered: {stats:?}");
    assert!(count(&stats, "cut") > 0, "some sections are cut to their budgets: {stats:?}");
    for ending in ["missing", "failed", "expired", "busy", "oversized", "late"] {
        assert_eq!(count(&stats, ending), 0, "{ending}: {stats:?}");
    }
    let (sections, cuts) = world.judged();
    assert!(sections > 50 && cuts > 0, "the referee judged sections and cuts: {sections}, {cuts}");
}

#[test]
fn content_past_what_a_read_brings_is_cut_by_its_source_and_told() {
    let settings = Settings { parts: 12, part_chars: 300, ..Settings::calm(2) };
    let stats = run(settings).stats();
    assert!(count(&stats, "source cut") > 0 && count(&stats, "cut") > 0, "{stats:?}");
    assert!(count(&stats, "over total") > 0, "briefs held to their total: {stats:?}");
    assert_eq!(count(&stats, "rendered"), stats.briefs, "{stats:?}");
}

#[test]
fn reads_that_fail_or_come_late_leave_sections_missing_or_fail_the_brief() {
    let settings =
        Settings { failures: 200, late: 300, lateness: Span::millis(0, 40_000), required: 400, ..Settings::calm(3) };
    let stats = run(settings).stats();
    for ending in ["missing", "failed", "expired", "late", "read failed"] {
        assert!(count(&stats, ending) > 0, "{ending}: {stats:?}");
    }
    assert_eq!(
        count(&stats, "rendered") + count(&stats, "failed") + count(&stats, "busy"),
        stats.briefs,
        "every brief answered once: {stats:?}"
    );
}

#[test]
fn briefs_past_the_limits_or_asked_too_fast_are_refused_at_the_entrance() {
    let settings = Settings {
        limits: Limits { briefs: 1, ..LIMITS },
        brief_gap: Span::millis(0, 50),
        oversized: 300,
        ..Settings::calm(4)
    };
    let stats = run(settings).stats();
    assert!(count(&stats, "busy") > 0 && count(&stats, "oversized") > 0, "{stats:?}");
}

#[test]
fn a_seed_replays_to_the_same_run() {
    let run = |seed: u64| {
        let world = run(Settings::random(seed));
        (world.trace().to_vec(), (world.stats(), world.now()))
    };
    let trace = assert_replays(7, 8, run);
    assert!(trace.len() > 100, "the world did something");
}

#[test]
fn facts_change_nothing() {
    for seed in 0..5 {
        let settings = Settings::random(seed);
        let none = run(Settings { limits: Limits { facts: 0, ..settings.limits }, ..settings });
        let many = run(Settings { limits: Limits { facts: 4096, ..settings.limits }, ..settings });
        assert!(none.trace() == many.trace(), "seed {seed}: the same run whatever facts are kept");
    }
}

#[test]
fn random_worlds_settle_with_every_ending_reached() {
    let mut endings = BTreeSet::new();
    let (mut sections, mut cuts) = (0, 0);
    for seed in 0..100 {
        let world = run(Settings::random(seed));
        endings.extend(world.stats().endings.keys().copied());
        let judged = world.judged();
        (sections, cuts) = (sections + judged.0, cuts + judged.1);
    }
    let missed: Vec<&str> = ENDINGS.iter().copied().filter(|ending| !endings.contains(ending)).collect();
    assert!(missed.is_empty(), "every ending was reached: {missed:?} were not");
    assert!(sections > 5000 && cuts > 500, "the referee judged sections and cuts: {sections}, {cuts}");
}
