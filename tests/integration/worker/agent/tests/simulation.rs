//! The agent sub-model in its world: scenarios, replay, and a sweep of random
//! worlds.

use std::collections::{BTreeMap, BTreeSet};

use temper_worker_model_agent::Limits;
use temper_worker_model_agent_tests::script::{self, Fates};
use temper_worker_model_agent_tests::{Settings, Span, Stats, World, client, tree};

const ITERATIONS: u32 = 400_000;

fn run(settings: &Settings) -> World {
    let mut world = World::new(*settings);
    world.run(ITERATIONS);
    world
}

fn count(map: &BTreeMap<&'static str, u32>, kind: &str) -> u32 {
    map.get(kind).copied().unwrap_or(0)
}

/// Runs that meet the `fates` given, and never wait long enough to park.
fn only(fates: Fates) -> Settings {
    let calm = Settings::calm(0);
    Settings { script: script::Script { fates, waits: 0, ..calm.script }, ..calm }
}

const NONE: Fates = Fates {
    ended: 0,
    parked: 0,
    failed: 0,
    crash: 0,
    hang: 0,
    overrun: 0,
    garbage: 0,
    duplicate: 0,
    trailing: 0,
    oversized: 0,
    deaf: 0,
    mute: 0,
};

#[test]
fn a_calm_world_finishes_every_run_and_settles() {
    let stats = run(&Settings::calm(1)).stats();
    assert_eq!(count(&stats.endings, "stopped"), 8, "{stats:?}");
    let finished =
        count(&stats.finishes, "ended") + count(&stats.finishes, "parked") + count(&stats.finishes, "failed");
    assert_eq!(finished, 8, "every run says how it finishes: {stats:?}");
    assert!(stats.faults.is_empty() && stats.breaches.is_empty(), "{stats:?}");
    assert_eq!(stats.tree.terminates, 0, "every agent exits on its own: {stats:?}");
    assert!(stats.client.calls > 0 && stats.client.events > 0 && stats.client.waiting > 0, "{stats:?}");
}

#[test]
fn spawns_beyond_the_slots_are_refused_as_busy() {
    let calm = Settings::calm(2);
    let settings = Settings {
        agent: Limits { agents: 1, ..calm.agent },
        client: client::Script { spacing: Span::millis(0, 100), ..calm.client },
        ..calm
    };
    let stats = run(&settings).stats();
    assert!(count(&stats.endings, "busy") > 0 && count(&stats.endings, "stopped") > 0, "{stats:?}");
    assert_eq!(stats.peak, 1, "one slot: {stats:?}");
}

#[test]
fn spawns_beyond_the_limits_are_refused_as_invalid() {
    let calm = Settings::calm(3);
    let settings = Settings { client: client::Script { invalid: 1000, ..calm.client }, ..calm };
    let stats = run(&settings).stats();
    assert_eq!(count(&stats.endings, "invalid"), 8, "{stats:?}");
    assert_eq!(stats.tree.spawns, 0, "{stats:?}");
}

#[test]
fn processes_that_cannot_be_spawned_are_gone_unstarted() {
    let calm = Settings::calm(4);
    let settings = Settings { tree: tree::Script { unspawned: 1000, ..calm.tree }, ..calm };
    let stats = run(&settings).stats();
    assert_eq!(count(&stats.endings, "unspawned"), 8, "{stats:?}");
    assert_eq!(stats.client.started, 0, "{stats:?}");
}

#[test]
fn a_cancelled_run_winds_down_and_its_finish_is_heard() {
    let calm = Settings::calm(5);
    let settings = Settings {
        client: client::Script { stops: 1000, stop_after: Span::millis(0, 5_000), ..calm.client },
        script: script::Script { steps: 30, ..calm.script },
        ..calm
    };
    let stats = run(&settings).stats();
    assert!(count(&stats.facts, "cancelled") > 0, "{stats:?}");
    assert!(count(&stats.finishes, "failed") > 0, "cancelled runs say so: {stats:?}");
    assert!(stats.faults.is_empty(), "nothing failed: {stats:?}");
    assert_eq!(count(&stats.endings, "stopped"), 8, "{stats:?}");
}

#[test]
fn an_ignored_cancel_is_terminated_then_killed() {
    let calm = Settings::calm(6);
    let settings = Settings {
        client: client::Script { stops: 1000, stop_after: Span::millis(0, 5_000), ..calm.client },
        script: script::Script { steps: 30, deaf_to_cancel: 1000, stubborn: 1000, ..calm.script },
        ..calm
    };
    let stats = run(&settings).stats();
    assert!(stats.tree.terminates > 0 && stats.tree.kills > 0, "{stats:?}");
    assert!(stats.tree.ignored > 0, "{stats:?}");
    assert_eq!(count(&stats.endings, "stopped"), 8, "{stats:?}");
}

#[test]
fn hung_runs_are_stopped_by_the_watchdog() {
    // Waits and inbound events stay on: a run that has read every event and
    // waits is not hung, and one that hangs after a wait is.
    let calm = Settings::calm(9);
    let settings = Settings {
        script: script::Script { fates: Fates { hang: 1, ..NONE }, ..calm.script },
        client: client::Script { events: 6, ..calm.client },
        ..calm
    };
    let stats = run(&settings).stats();
    let caught = count(&stats.paths, "silence caught");
    assert!(caught > 0, "{stats:?}");
    assert!(count(&stats.faults, "no progress") >= caught, "{stats:?}");
    assert_eq!(stats.faults.len(), 1, "and nothing else: {stats:?}");
}

#[test]
fn a_wait_that_crosses_an_inbound_event_does_not_pause_the_watchdog() {
    // Slow pipes: the run often says it waits while an event is on its way.
    let calm = Settings::calm(11);
    let settings = Settings {
        tree: tree::Script { pipe: Span::millis(300, 3_000), ..calm.tree },
        script: script::Script { fates: Fates { hang: 1, ..NONE }, waits: 400, steps: 12, ..calm.script },
        client: client::Script { spawns: 24, events: 8, event_gap: Span::millis(100, 3_000), ..calm.client },
        ..calm
    };
    let stats = run(&settings).stats();
    assert!(count(&stats.paths, "silence caught") > 0, "{stats:?}");
    assert_eq!(stats.faults.len(), 1, "{stats:?}");
}

#[test]
fn runs_that_close_their_output_hang_up_live_or_cancelled() {
    let calm = only(Fates { mute: 1, ended: 1, ..NONE });
    let settings = Settings {
        client: client::Script { stops: 500, stop_after: Span::millis(0, 5_000), ..calm.client },
        script: script::Script { steps: 20, mute: 1000, ..calm.script },
        ..calm
    };
    let stats = run(&settings).stats();
    assert!(count(&stats.paths, "hangup while live") > 0, "{stats:?}");
    assert!(count(&stats.paths, "hangup while cancelled") > 0, "{stats:?}");
    assert!(count(&stats.faults, "exited") > 0, "{stats:?}");
}

#[test]
fn withdrawn_calls_are_answered_once() {
    let calm = Settings::calm(10);
    let settings = Settings {
        client: client::Script { slow: 500, ..calm.client },
        script: script::Script { steps: 20, blocking: 1000, call_deadline: Span::millis(1_000, 5_000), ..calm.script },
        ..calm
    };
    let stats = run(&settings).stats();
    assert!(stats.client.withdrawn > 0, "{stats:?}");
    assert!(stats.faults.is_empty(), "{stats:?}");
}

#[test]
fn runs_that_keep_making_progress_are_stopped_only_by_their_wall_time() {
    let mut settings = only(Fates { overrun: 1, ..NONE });
    settings.agent.wall_time = temper_lib::Duration::from_secs(200);
    // Some wind down when cancelled; the rest are faulted past the grace.
    settings.script.deaf_to_cancel = 500;
    let stats = run(&settings).stats();
    let started = stats.client.started;
    assert!(started > 0, "{stats:?}");
    assert_eq!(count(&stats.facts, "overdue"), started, "every run is cancelled for its wall time: {stats:?}");
    let (failed, faulted) = (count(&stats.finishes, "failed"), count(&stats.faults, "wall time"));
    assert!(failed > 0 && faulted > 0, "{stats:?}");
    assert_eq!(failed + faulted, started, "and says it was cancelled, or is faulted for it: {stats:?}");
    assert_eq!(count(&stats.faults, "no progress"), 0, "{stats:?}");
}

#[test]
fn an_agent_that_exits_without_a_word_has_failed() {
    let calm = only(Fates { crash: 1, ..NONE });
    let settings = Settings { tree: tree::Script { children: 2, holding: 500, lingering: 500, ..calm.tree }, ..calm };
    let stats = run(&settings).stats();
    assert!(stats.client.started > 0, "{stats:?}");
    assert_eq!(count(&stats.faults, "exited"), stats.client.started, "{stats:?}");
    assert!(stats.tree.orphans > 0, "children outlived their agent: {stats:?}");
}

#[test]
fn an_agent_that_stops_reading_its_channel_is_drained() {
    let calm = only(Fates { deaf: 1, ..NONE });
    let settings =
        Settings { client: client::Script { events: 6, event_gap: Span::millis(0, 2_000), ..calm.client }, ..calm };
    let stats = run(&settings).stats();
    assert!(stats.tree.unsent > 0, "{stats:?}");
    assert_eq!(count(&stats.endings, "stopped"), 8, "{stats:?}");
}

#[test]
fn every_breach_of_the_channels_rules_is_caught() {
    let calm = only(Fates { garbage: 1, duplicate: 1, trailing: 1, oversized: 1, ..NONE });
    let settings = Settings { client: client::Script { spawns: 40, ..calm.client }, ..calm };
    let stats = run(&settings).stats();
    for kind in ["malformed", "reused name", "withdrawn twice", "after the finish", "oversized"] {
        assert!(count(&stats.breaches, kind) > 0, "{kind}: {stats:?}");
    }
    assert!(count(&stats.faults, "rules") > 0, "{stats:?}");
}

#[test]
fn slow_answers_and_waits_pause_the_watchdog() {
    let calm = Settings::calm(7);
    let settings = Settings {
        client: client::Script { slow: 1000, ..calm.client },
        script: script::Script { steps: 20, blocking: 1000, ..calm.script },
        ..calm
    };
    let stats = run(&settings).stats();
    assert!(stats.client.slow > 0 && stats.client.waiting > 0, "{stats:?}");
    assert!(stats.faults.is_empty(), "{stats:?}");
}

#[test]
fn children_that_outlive_their_agent_are_terminated_then_killed() {
    let calm = Settings::calm(8);
    let settings = Settings { tree: tree::Script { children: 2, lingering: 1000, stubborn: 500, ..calm.tree }, ..calm };
    let stats = run(&settings).stats();
    assert!(stats.tree.orphans > 0 && stats.tree.terminates > 0 && stats.tree.kills > 0, "{stats:?}");
    assert_eq!(count(&stats.endings, "stopped"), 8, "{stats:?}");
}

#[test]
fn a_seed_replays_to_the_same_run() {
    let trace = temper_world::assert_replays(11, 12, |seed| {
        let world = run(&Settings::rough(seed));
        (world.trace().to_vec(), (world.stats(), world.now()))
    });
    assert!(trace.len() > 100, "the world did something");
}

#[test]
fn facts_change_nothing_when_none_are_kept() {
    let settings = Settings::rough(13);
    let kept = run(&settings);
    let none = run(&Settings { agent: Limits { facts: 0, ..settings.agent }, ..settings });
    assert_eq!(kept.trace(), none.trace(), "nothing depends on whether a fact is kept");
    assert!(none.stats().facts_lost > 0);
}

#[test]
fn random_worlds_settle_and_reach_every_ending() {
    let mut reached = BTreeSet::new();
    for seed in 0..300 {
        let rough = Settings::rough(seed);
        // Every third world has slow pipes, so waits cross inbound events.
        let pipe = if seed.is_multiple_of(3) { Span::millis(300, 3_000) } else { rough.tree.pipe };
        let stats: Stats = run(&Settings { tree: tree::Script { pipe, ..rough.tree }, ..rough }).stats();
        let maps = [
            ("end", &stats.endings),
            ("finish", &stats.finishes),
            ("fault", &stats.faults),
            ("breach", &stats.breaches),
            ("bounce", &stats.bounces),
            ("path", &stats.paths),
        ];
        for (what, map) in maps {
            for (kind, count) in map {
                if *count > 0 {
                    reached.insert(format!("{what}: {kind}"));
                }
            }
        }
        let paths = [
            ("busy answers", stats.busy),
            ("answers too large", stats.too_large),
            ("orphans", stats.tree.orphans),
            ("kills", stats.tree.kills),
            ("ignored terminates", stats.tree.ignored),
            ("unsent", stats.tree.unsent),
            ("slow answers", stats.client.slow),
            ("withdrawn", stats.client.withdrawn),
            ("spawns past their deadline", stats.tree.late_spawns),
            ("stale handles", stats.client.stale),
        ];
        for (path, count) in paths {
            if count > 0 {
                reached.insert(path.to_string());
            }
        }
    }
    let every = [
        "end: busy",
        "end: invalid",
        "end: unspawned",
        "end: stopped",
        "finish: ended",
        "finish: parked",
        "finish: failed",
        "fault: exited",
        "fault: rules",
        "fault: no progress",
        "fault: wall time",
        "breach: malformed",
        "breach: oversized",
        "breach: reused name",
        "breach: after the finish",
        "breach: withdrawn twice",
        "path: hangup while live",
        "path: hangup while cancelled",
        "path: finish while terminating",
        "path: silence caught",
        "withdrawn",
        "spawns past their deadline",
        "bounce: too large",
        "bounce: full",
        "bounce: ending",
        "busy answers",
        "answers too large",
        "orphans",
        "kills",
        "ignored terminates",
        "unsent",
        "slow answers",
        "stale handles",
    ];
    let missing: Vec<&str> = every.iter().copied().filter(|path| !reached.contains(*path)).collect();
    assert!(missing.is_empty(), "every ending and path is reached: {missing:?} are not");
}
