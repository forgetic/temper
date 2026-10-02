//! The plan in its world: scenarios, replay, and a sweep of random worlds.

use std::collections::BTreeSet;

use temper_engine_model_plan_tests::script::Script;
use temper_engine_model_plan_tests::{Settings, Stats, World};
use temper_world::assert_replays;

const ITERATIONS: u32 = 1_000_000;

fn run(settings: &Settings) -> World {
    let mut world = World::new(*settings);
    world.run(ITERATIONS);
    world
}

fn path(stats: &Stats, path: &str) -> u32 {
    stats.paths.get(path).copied().unwrap_or(0)
}

fn ending(stats: &Stats, ending: &str) -> u32 {
    stats.endings.get(ending).copied().unwrap_or(0)
}

#[test]
fn a_calm_world_lands_every_change_and_ends_every_goal_done() {
    let stats = run(&Settings::calm(1)).stats();
    assert_eq!(ending(&stats, "goal: done"), 2, "{stats:?}");
    assert_eq!(stats.endings.len(), 1, "and nothing else: {stats:?}");
    assert!(path(&stats, "run: produce") > 0, "{stats:?}");
    assert_eq!(path(&stats, "merged"), path(&stats, "pull request opened"), "{stats:?}");
    assert_eq!(path(&stats, "run: produce"), path(&stats, "merged"), "no change needed a repair: {stats:?}");
    assert!(path(&stats, "run: work") > 0, "{stats:?}");
    assert_eq!(path(&stats, "proposal accepted"), 2, "{stats:?}");
}

#[test]
fn changes_are_repaired_until_they_land_or_run_out_of_repairs() {
    let calm = Settings::calm(2);
    let settings = Settings {
        goals: 4,
        script: Script { changes: 800, ci_fails: 300, changes_asked: 300, conflicts: 200, pushes: 500, ..calm.script },
        ..calm
    };
    let stats = run(&settings).stats();
    for repair in ["CI failed", "changes asked for", "conflicts", "base moved"] {
        assert!(path(&stats, &format!("run: repair, {repair}")) > 0, "{repair}: {stats:?}");
    }
    assert!(path(&stats, "merged") > 0, "{stats:?}");
    assert!(
        ending(&stats, "goal: done") + ending(&stats, "goal: held, repairs") + ending(&stats, "goal: held, rebases")
            == 4,
        "{stats:?}"
    );
}

#[test]
fn growth_beyond_the_envelope_waits_for_a_persons_acceptance() {
    let calm = Settings::calm(3);
    let settings =
        Settings { goals: 3, script: Script { grows: 1000, growth: 1000, beyond: 600, ..calm.script }, ..calm };
    let stats = run(&settings).stats();
    assert!(path(&stats, "growth: within") > 0, "{stats:?}");
    assert!(path(&stats, "growth: accepted beyond") > 0, "{stats:?}");
    assert_eq!(ending(&stats, "goal: done"), 3, "{stats:?}");
}

#[test]
fn invalid_plans_are_feedback_and_fixed() {
    let calm = Settings::calm(4);
    let settings = Settings { script: Script { invalid: 1000, ..calm.script }, ..calm };
    let stats = run(&settings).stats();
    assert_eq!(path(&stats, "invalid"), 2, "{stats:?}");
    assert_eq!(ending(&stats, "goal: done"), 2, "{stats:?}");
}

#[test]
fn an_application_interrupted_by_a_restart_finds_what_it_made() {
    let calm = Settings::calm(5);
    let settings = Settings {
        restarts: 500,
        script: Script { grows: 500, growth: 500, tasks: 500, ..calm.script },
        tasks: 2,
        ..calm
    };
    let stats = run(&settings).stats();
    assert!(path(&stats, "engine restarts as it applies") > 0, "{stats:?}");
    assert!(path(&stats, "found by its key") > 0, "{stats:?}");
    assert_eq!(ending(&stats, "goal: done"), 2, "{stats:?}");
}

#[test]
fn a_rough_world_settles() {
    let stats = run(&Settings::rough(6)).stats();
    assert!(stats.checked > 0, "{stats:?}");
    assert!(!stats.endings.is_empty(), "{stats:?}");
}

#[test]
fn a_seed_replays_to_the_same_run() {
    let trace = assert_replays(7, 8, |seed| {
        let world = run(&Settings::rough(seed));
        (world.trace().to_vec(), (world.stats(), world.now()))
    });
    assert!(trace.len() > 50, "the world did something: {} lines", trace.len());
}

/// Every way a goal or a task ends.
const ENDINGS: [&str; 10] = [
    "goal: done",
    "goal: held, rejected",
    "goal: held, repairs",
    "goal: held, rebases",
    "goal: held, pull request closed",
    "goal: held, escalated",
    "goal: held, stalled",
    "goal: held, attempts",
    "task: done",
    "task: held, escalated",
];

/// The paths a sweep takes, each at least once.
const PATHS: [&str; 32] = [
    "run: work",
    "run: produce",
    "run: review",
    "run: turn",
    "run: repair, CI failed",
    "run: repair, changes asked for",
    "run: repair, base moved",
    "run: repair, conflicts",
    "run: resumes a snapshot",
    "outcome: finished",
    "outcome: release",
    "outcome: tasks",
    "invalid",
    "invalid once accepted",
    "stale: moved",
    "stale: landed",
    "stale: closed",
    "proposal rejected",
    "growth: within",
    "growth: accepted beyond",
    "engine restarts",
    "engine restarts as it applies",
    "engine restarts between the goal's record and the step's",
    "found by its key",
    "pull request reopened",
    "decision: accepted",
    "decision: rejected",
    "released by a person",
    "released by its goal's session",
    "ci: silent",
    "inbox: noise before a message",
    "woken by a message behind many events",
];

#[test]
fn random_worlds_settle_and_reach_every_ending() {
    let mut endings = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for seed in 100..300 {
        let stats = run(&Settings::rough(seed)).stats();
        for ending in stats.endings.keys() {
            endings.insert(ending.clone());
        }
        for path in stats.paths.keys() {
            paths.insert(path.clone());
        }
    }
    for ending in ENDINGS {
        assert!(endings.contains(ending), "some world ends {ending}: {endings:?}");
    }
    for path in PATHS {
        assert!(paths.contains(path), "some world takes {path}: {paths:?}");
    }
}
