//! The views in their world: scenarios, replay, and a sweep of random worlds.

use temper_engine_model_views::Limits;
use temper_engine_model_views_tests::{LIMITS, Settings, Span, Stats, World};
use temper_lib::Duration;
use temper_world::assert_replays;

const ITERATIONS: u32 = 400_000;

fn run(settings: &Settings) -> World {
    let mut world = World::new(*settings);
    world.run(ITERATIONS);
    world
}

fn count(stats: &Stats, ending: &str) -> u32 {
    stats.endings.get(ending).copied().unwrap_or(0)
}

#[test]
fn a_calm_world_streams_and_traces_everything_and_settles() {
    let world = run(&Settings::calm(1));
    let stats = world.stats();
    assert!(count(&stats, "delivered") > 100, "the watchers were delivered to: {stats:?}");
    assert!(count(&stats, "appended") > 10 && count(&stats, "expired") > 0, "the store kept and forgot: {stats:?}");
    assert!(count(&stats, "unwatched") > 0, "{stats:?}");
    for ending in [
        "missed",
        "undelivered",
        "hung",
        "busy",
        "unknown",
        "unfollowed",
        "oversized snapshot",
        "append failed",
        "expire failed",
        "at once",
        "oversized",
        "late",
        "lost",
        "reused",
        "restarted",
        "closed",
        "burst",
    ] {
        assert_eq!(count(&stats, ending), 0, "{ending}: {stats:?}");
    }
    let (chunks, deliveries, missed, records) = world.judged();
    assert!(chunks > 200 && deliveries > 100 && records > 50, "the referee judged: {:?}", world.judged());
    assert_eq!(missed, 0);
}

#[test]
fn slow_watchers_miss_some_and_are_told_so() {
    let settings = Settings {
        slow: 600,
        slow_latency: Span::millis(200, 1500),
        report_gap: Span::millis(1, 40),
        watch_for: Span::millis(5000, 40_000),
        ..Settings::calm(2)
    };
    let world = run(&settings);
    let stats = world.stats();
    assert!(count(&stats, "missed") > 0, "{stats:?}");
    let (_, _, missed, _) = world.judged();
    assert!(missed > 0, "the referee counted what was missed");
}

#[test]
fn watches_past_the_limits_or_of_runs_not_followed_are_refused_at_the_entrance() {
    let settings = Settings {
        limits: Limits { watchers: 2, runs: 1, ..LIMITS },
        watches: 40,
        watch_gap: Span::millis(1, 400),
        run_gap: Span::millis(100, 1000),
        stale: 200,
        ..Settings::calm(3)
    };
    let stats = run(&settings).stats();
    for ending in ["busy", "unknown", "unfollowed"] {
        assert!(count(&stats, ending) > 0, "{ending}: {stats:?}");
    }
}

#[test]
fn a_store_that_is_slow_and_fails_loses_traces_and_still_forgets_them() {
    let settings = Settings {
        limits: Limits { appends: 1, records: 3, ..LIMITS },
        store_latency: Span::millis(50, 2000),
        store_failures: 300,
        instant: 300,
        report_gap: Span::millis(1, 50),
        ..Settings::calm(4)
    };
    let stats = run(&settings).stats();
    for ending in ["append failed", "partly kept", "expire failed", "lost", "expired", "at once"] {
        assert!(count(&stats, ending) > 0, "{ending}: {stats:?}");
    }
}

#[test]
fn a_retention_shorter_than_the_store_takes_still_leaves_nothing_kept() {
    let (mut appended, mut expired) = (0, 0);
    for seed in 0..40 {
        let settings = Settings {
            limits: Limits {
                retention: Duration::from_millis(100),
                sweep: Duration::from_millis(300),
                flush: Duration::from_millis(1000),
                ..LIMITS
            },
            // A few runs reporting sparsely, so that a batch waits its flush
            // past the retention, and a store slow enough to be expiring
            // when it goes.
            runs: 2,
            reports: 10,
            report_gap: Span::millis(100, 2000),
            store_latency: Span::millis(200, 1500),
            store_failures: 100,
            ..Settings::calm(seed)
        };
        let stats = run(&settings).stats();
        (appended, expired) = (appended + count(&stats, "appended"), expired + count(&stats, "expired"));
    }
    assert!(appended > 100 && expired > 100, "the store kept and forgot: {appended}, {expired}");
}

#[test]
fn deliveries_a_stream_does_not_take_are_told_as_missed() {
    let settings = Settings { undelivered: 200, hang: 50, ..Settings::calm(7) };
    let world = run(&settings);
    let stats = world.stats();
    for ending in ["undelivered", "hung", "missed"] {
        assert!(count(&stats, ending) > 0, "{ending}: {stats:?}");
    }
}

#[test]
fn watches_begun_together_under_tokens_reused_are_each_answered_and_ended() {
    let settings = Settings {
        limits: Limits { watchers: 4, ..LIMITS },
        burst: 500,
        people: 4,
        watch_for: Span::millis(1, 3000),
        watches: 60,
        ..Settings::calm(8)
    };
    let stats = run(&settings).stats();
    for ending in ["reused", "busy", "unwatched"] {
        assert!(count(&stats, ending) > 0, "{ending}: {stats:?}");
    }
}

#[test]
fn watches_and_store_operations_that_end_within_an_iteration_wait_for_free_slots() {
    let settings = Settings {
        limits: Limits { watchers: 1, appends: 1, records: 1, ..LIMITS },
        burst: 1000,
        closed: 1000,
        watch_gap: Span::millis(1, 500),
        watches: 80,
        report_burst: 1000,
        instant: 1000,
        ..Settings::calm(9)
    };
    let stats = run(&settings).stats();
    for ending in ["closed", "busy", "burst", "at once"] {
        assert!(count(&stats, ending) > 0, "{ending}: {stats:?}");
    }
}

#[test]
fn restarts_drop_every_watch_and_still_forget_what_the_store_keeps() {
    let mut restarted = 0;
    for seed in 0..10 {
        let settings = Settings {
            restarts: 3,
            restart_at: Span::millis(0, 60_000),
            people: 3,
            store_failures: 100,
            ..Settings::calm(seed)
        };
        restarted += count(&run(&settings).stats(), "restarted");
    }
    assert_eq!(restarted, 30);
}

#[test]
fn reports_past_the_limits_or_after_their_run_are_dropped() {
    let settings = Settings { oversized: 100, late: 500, ..Settings::calm(5) };
    let stats = run(&settings).stats();
    assert!(count(&stats, "oversized") > 0 && count(&stats, "late") > 0, "{stats:?}");
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
