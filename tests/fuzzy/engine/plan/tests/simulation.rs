//! The engine's plan child domain at random: many rough worlds, each settled,
//! every ending and every path reached among them.

use std::collections::BTreeSet;

use temper_engine_domain_plan_tests::{Settings, World};

const ITERATIONS: u32 = 1_000_000;

fn run(settings: &Settings) -> World {
    let mut world = World::new(*settings);
    world.run(ITERATIONS);
    world
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
