//! The host in its world: scenarios, replay, and a sweep of random worlds.

use std::collections::BTreeSet;

use temper_lib::Duration;
use temper_worker_model_host::Limits;
use temper_worker_model_host_tests::engine::ENDINGS;
use temper_worker_model_host_tests::{Outage, Settings, Span, Stats, World};

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
fn a_calm_world_ends_or_parks_every_run_and_settles() {
    let world = run(&Settings::calm(1));
    let stats = world.stats();
    let (ended, parked) = (count(&stats, "ended"), count(&stats, "parked"));
    assert!(ended > 0 && parked > 0, "runs end, or park when idle: {stats:?}");
    assert_eq!(ended + parked, 8, "and nothing else: {stats:?}");
    assert!(stats.parent.relays > 0 && stats.parent.pushes > 0 && stats.parent.yields > 0, "{stats:?}");
    assert!(stats.parent.saves > 0, "work that did not land is saved: {stats:?}");
    assert_eq!(stats.parent.releases, 8, "every workspace is released: {stats:?}");
    assert!(stats.engine.events > 0, "inbound events went down: {stats:?}");
}

#[test]
fn assignments_beyond_the_slots_are_refused_as_busy() {
    let calm = Settings::calm(2);
    let settings = Settings {
        host: Limits { slots: 1, ..calm.host },
        engine: temper_worker_model_host_tests::engine::Script { spacing: Span::millis(0, 100), ..calm.engine },
        ..calm
    };
    let stats = run(&settings).stats();
    assert!(count(&stats, "refused: busy") > 0, "{stats:?}");
    assert!(count(&stats, "ended") > 0, "{stats:?}");
    assert_eq!(stats.peak, 1, "one slot: {stats:?}");
}

#[test]
fn assignments_beyond_the_limits_are_refused_as_invalid_and_never_admitted() {
    let calm = Settings::calm(3);
    let settings =
        Settings { engine: temper_worker_model_host_tests::engine::Script { invalid: 1000, ..calm.engine }, ..calm };
    let stats = run(&settings).stats();
    assert_eq!(count(&stats, "refused: invalid"), 8, "{stats:?}");
    assert_eq!(stats.parent.prepares, 0, "{stats:?}");
}

#[test]
fn cancelled_runs_are_stopped_saved_and_their_calls_answered_as_unavailable() {
    let calm = Settings::calm(4);
    let settings = Settings {
        engine: temper_worker_model_host_tests::engine::Script {
            cancels: 1000,
            cancel_after: Span::millis(0, 20_000),
            ..calm.engine
        },
        parent: temper_worker_model_host_tests::parent::Script { relays: 600, late: 1000, ..calm.parent },
        ..calm
    };
    let stats = run(&settings).stats();
    assert!(count(&stats, "failed: cancelled, engine") > 0, "{stats:?}");
    assert!(stats.unavailable > 0, "{stats:?}");
    assert!(stats.engine.unanswered > 0 || stats.parent.late_calls > 0, "{stats:?}");
}

#[test]
fn stale_attempts_change_nothing() {
    let calm = Settings::calm(5);
    let settings =
        Settings { engine: temper_worker_model_host_tests::engine::Script { stale: 1000, ..calm.engine }, ..calm };
    let stats = run(&settings).stats();
    assert!(stats.stale > 8, "{stats:?}");
    assert_eq!(count(&stats, "ended") + count(&stats, "parked"), 8, "{stats:?}");
}

#[test]
fn losing_contact_past_the_grace_cancels_every_run_and_saves_their_work_first() {
    let calm = Settings::calm(6);
    let settings = Settings {
        engine: temper_worker_model_host_tests::engine::Script {
            assignments: 4,
            spacing: Span::millis(0, 1_000),
            ..calm.engine
        },
        parent: temper_worker_model_host_tests::parent::Script { steps: 30, ..calm.parent },
        outage: Some(Outage {
            at: Span::millis(5_000, 5_000),
            length: Span::millis(60_000, 60_000),
            grace: Duration::from_secs(10),
        }),
        ..calm
    };
    let stats = run(&settings).stats();
    assert_eq!(stats.cancel_alls, 1, "{stats:?}");
    assert!(count(&stats, "failed: cancelled, contact") > 0, "{stats:?}");
    assert!(stats.parent.saves > 0, "{stats:?}");
    assert_eq!(stats.reports, 1, "{stats:?}");
}

#[test]
fn losing_contact_within_the_grace_keeps_the_runs_and_reports_them() {
    let calm = Settings::calm(7);
    let settings = Settings {
        engine: temper_worker_model_host_tests::engine::Script {
            assignments: 4,
            spacing: Span::millis(0, 1_000),
            ..calm.engine
        },
        parent: temper_worker_model_host_tests::parent::Script { steps: 30, ..calm.parent },
        outage: Some(Outage {
            at: Span::millis(5_000, 5_000),
            length: Span::millis(5_000, 5_000),
            grace: Duration::from_secs(10),
        }),
        ..calm
    };
    let stats = run(&settings).stats();
    assert_eq!((stats.cancel_alls, stats.reports), (0, 1), "{stats:?}");
    assert_eq!(count(&stats, "ended") + count(&stats, "parked"), 4, "kept, and none failed: {stats:?}");
}

#[test]
fn shutdown_cancels_every_run_and_takes_no_more() {
    let calm = Settings::calm(8);
    let settings = Settings {
        parent: temper_worker_model_host_tests::parent::Script { steps: 30, ..calm.parent },
        shutdown: Some(Span::millis(30_000, 30_000)),
        ..calm
    };
    let stats = run(&settings).stats();
    assert_eq!(stats.cancel_alls, 1, "{stats:?}");
    assert!(count(&stats, "failed: cancelled, shutdown") > 0, "{stats:?}");
    assert!(count(&stats, "refused: busy") > 0, "assignments after shutdown are refused: {stats:?}");
}

/// A rough world, varied by seed: some shut down, some lose contact for
/// longer than the grace, some let runs make several calls at once and hold
/// several inbound events.
fn rough(seed: u64) -> Settings {
    let rough = Settings::rough(seed);
    let shutdown = if seed.is_multiple_of(4) { Some(Span::millis(20_000, 120_000)) } else { None };
    let host = if seed.is_multiple_of(2) { rough.host } else { Limits { run_calls: 3, held: 3, ..rough.host } };
    Settings { shutdown, host, ..rough }
}

#[test]
fn a_seed_replays_to_the_same_run() {
    let trace = temper_world::assert_replays(11, 12, |seed| {
        let world = run(&rough(seed));
        (world.trace().to_vec(), (world.stats(), world.now()))
    });
    assert!(trace.len() > 100, "the world did something");
}

#[test]
fn facts_change_nothing_when_none_are_kept() {
    let settings = rough(13);
    let kept = run(&settings);
    let none = run(&Settings { host: Limits { facts: 0, ..settings.host }, ..settings });
    assert_eq!(kept.trace(), none.trace(), "nothing depends on whether a fact is kept");
    assert!(none.stats().facts_lost > 0);
}

#[test]
fn random_worlds_settle_and_reach_every_ending() {
    let mut reached = BTreeSet::new();
    let mut seen = BTreeSet::new();
    for seed in 0..300 {
        let stats = run(&rough(seed)).stats();
        for ending in stats.endings.keys() {
            reached.insert(*ending);
        }
        let paths = [
            ("busy calls", stats.busy),
            ("calls unavailable", stats.unavailable),
            ("late calls", stats.parent.late_calls),
            ("relays unanswered", stats.engine.unanswered),
            ("events too large", stats.engine.too_large),
            ("events past the hold", stats.engine.full),
            ("events to a run ending", stats.engine.ending),
            ("stale messages", stats.stale),
            ("reports", stats.reports),
            ("cancels of every run", stats.cancel_alls),
            ("runs forgotten on reconnecting", stats.engine.forgotten),
            ("requests to agents gone", stats.parent.dropped),
            ("saves with a branch moved", stats.parent.saves_moved),
            ("saves with a push failed", stats.parent.saves_failed),
        ];
        for (path, count) in paths {
            if count > 0 {
                seen.insert(path);
            }
        }
        for path in stats.paths.keys() {
            seen.insert(path);
        }
    }
    let missing: Vec<&str> = ENDINGS.iter().copied().filter(|ending| !reached.contains(ending)).collect();
    assert!(missing.is_empty(), "every ending is reached: {missing:?} are not");
    let paths = [
        "duplicate assignments",
        "oversized snapshots",
        "endings said during a stop",
        "pushes settled during a stop",
        "cancels as a workspace was prepared",
        "cancels as a workspace failed to prepare",
        "cancels as an agent failed to start",
        "cancels as an agent started",
    ];
    let missing: Vec<&str> = paths.iter().copied().filter(|path| !seen.contains(path)).collect();
    assert!(missing.is_empty(), "every path is taken: {missing:?} are not");
    assert_eq!(seen.len(), 14 + paths.len(), "every path is taken: only {seen:?} are");
}
