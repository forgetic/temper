//! The brief in its world: scenarios, replay, and a sweep of random worlds.

use temper_engine_domain_brief::Limits;
use temper_engine_domain_brief_tests::{LIMITS, Settings, Span, Stats, World};
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
    for ending in ["missing", "failed by read", "failed by deadline", "expired", "busy", "oversized", "late"] {
        assert_eq!(count(&stats, ending), 0, "{ending}: {stats:?}");
    }
    let (sections, cuts) = world.judged();
    assert!(sections > 50 && cuts > 0, "the referee judged sections and cuts: {sections}, {cuts}");
}

#[test]
fn content_past_what_a_read_brings_is_cut_by_its_source_and_told() {
    let settings = Settings { parts: 12, part_chars: 300, ..Settings::calm(2) };
    let stats = run(settings).stats();
    for ending in ["source cut", "cut", "over total"] {
        assert!(count(&stats, ending) > 0, "{ending}: {stats:?}");
    }
    assert_eq!(count(&stats, "rendered"), stats.briefs, "{stats:?}");
}

#[test]
fn lists_longer_than_a_source_may_name_are_cut_and_told() {
    let settings = Settings { long: 500, ..Settings::calm(5) };
    let stats = run(settings).stats();
    assert!(count(&stats, "items cut") > 0, "{stats:?}");
    assert_eq!(count(&stats, "oversized"), 0, "{stats:?}");
}

#[test]
fn reads_that_fail_or_come_late_leave_sections_missing_or_fail_the_brief() {
    let settings = Settings {
        failures: 200,
        late: 300,
        lateness: Span::millis(0, 40_000),
        required: 400,
        ties: 100,
        ..Settings::calm(3)
    };
    let stats = run(settings).stats();
    for ending in ["missing", "failed by read", "failed by deadline", "expired", "late", "read failed", "tie"] {
        assert!(count(&stats, ending) > 0, "{ending}: {stats:?}");
    }
    let answered = count(&stats, "rendered") + count(&stats, "failed by read") + count(&stats, "failed by deadline");
    assert_eq!(answered + count(&stats, "busy"), stats.briefs, "every brief answered once: {stats:?}");
}

#[test]
fn briefs_past_the_limits_or_asked_too_fast_are_refused_and_told_of_room() {
    let settings = Settings {
        limits: Limits { briefs: 1, ..LIMITS },
        brief_gap: Span::millis(0, 50),
        oversized: 300,
        ..Settings::calm(4)
    };
    let stats = run(settings).stats();
    for ending in ["busy", "room", "oversized"] {
        assert!(count(&stats, ending) > 0, "{ending}: {stats:?}");
    }
}

#[test]
fn reads_that_outlive_their_briefs_hold_room_until_they_end() {
    // One brief at a time, whose reads mostly come back long after its time
    // has run out: their room is held until they do.
    let settings = Settings {
        limits: Limits { briefs: 1, sections: 3, ..LIMITS },
        briefs: 80,
        brief_gap: Span::millis(3000, 6000),
        late: 900,
        lateness: Span::millis(30_000, 60_000),
        ..Settings::calm(6)
    };
    let stats = run(settings).stats();
    for ending in ["expired", "late", "busy", "room"] {
        assert!(count(&stats, ending) > 0, "{ending}: {stats:?}");
    }
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
