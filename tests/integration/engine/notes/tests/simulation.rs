//! The notes in their world: scenarios, replay, and a sweep of random worlds.

use std::collections::BTreeSet;

use temper_engine_domain_notes::{Limits, Scope};
use temper_engine_domain_notes_tests::{LIMITS, RUNS, Settings, Stats, World};
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
fn a_calm_world_answers_every_call_and_settles() {
    let world = run(Settings::calm(1));
    let stats = world.stats();
    let answered: u32 = stats
        .endings
        .iter()
        .filter(|(ending, _)| !["cut", "unread", "read failed", "evicted"].contains(ending))
        .map(|(_, count)| count)
        .sum();
    assert_eq!(answered, stats.calls, "every call answered once: {stats:?}");
    for ending in ["indexed", "found", "recalled", "noted"] {
        assert!(count(&stats, ending) > 0, "{ending}: {stats:?}");
    }
    assert_eq!(count(&stats, "busy") + count(&stats, "oversized") + count(&stats, "unavailable"), 0, "{stats:?}");
    let (lines, entries) = world.judged();
    assert!(lines > 0 && entries > 0, "the referee judged lines and entries: {lines}, {entries}");
}

#[test]
fn once_people_stop_the_index_holds_what_the_wiki_holds() {
    // Room for every page and every line, and every edit hinted.
    let limits = Limits { entries: 8, lines: 32, scopes: 5, ..LIMITS };
    for seed in 0..10 {
        let mut world = run(Settings { limits, ..Settings::calm(seed) });
        for run in RUNS {
            let lines = world.index(run, ITERATIONS);
            let mut scopes = vec![Scope::Repository(run.repository), Scope::Deployment];
            if let Some(goal) = run.goal {
                scopes.push(Scope::Goal { repository: goal.repository, number: goal.number });
            }
            for scope in scopes {
                let indexed: BTreeSet<Vec<u8>> =
                    lines.iter().filter(|(of, _)| *of == scope).map(|(_, name)| name.clone()).collect();
                let pages: BTreeSet<Vec<u8>> = world.pages(scope).into_iter().collect();
                assert_eq!(indexed, pages, "seed {seed}: the index of {scope:?} is the wiki's");
            }
        }
    }
}

#[test]
fn a_wiki_that_fails_and_answers_late_still_answers_every_call() {
    let mut endings = BTreeSet::new();
    for seed in 0..8 {
        let settings = Settings {
            failures: 300,
            late: 300,
            lateness: temper_world::Span::millis(0, 20_000),
            hinted: 300,
            ..Settings::calm(seed)
        };
        let stats = run(settings).stats();
        assert!(stats.failures > 0 && stats.lates > 0, "{stats:?}");
        endings.extend(stats.endings.keys().copied());
    }
    for ending in ["recalled", "read failed", "unavailable"] {
        assert!(endings.contains(ending), "{ending}: a read or a write that failed is told: {endings:?}");
    }
}

#[test]
fn more_scopes_than_are_kept_evict_the_least_recently_used_or_wait_at_the_entrance() {
    let settings = Settings {
        limits: Limits { scopes: 3, calls: 3, ..LIMITS },
        call_gap: temper_world::Span::millis(0, 50),
        ..Settings::calm(4)
    };
    let stats = run(settings).stats();
    assert!(count(&stats, "evicted") > 0, "{stats:?}");
    assert!(count(&stats, "busy") > 0, "{stats:?}");
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
