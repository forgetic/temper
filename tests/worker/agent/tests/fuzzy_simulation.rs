//! The worker's agent child domain at random: many random worlds, each settled,
//! every ending reached among them.

use std::collections::BTreeSet;

use temper_worker_agent_world::{Settings, Span, Stats, World, tree};

const ITERATIONS: u32 = 400_000;

fn run(settings: &Settings) -> World {
    let mut world = World::new(*settings);
    world.run(ITERATIONS);
    world
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
