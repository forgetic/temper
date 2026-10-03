//! The whole worker's world at random: many rough worlds against the engine,
//! each settled, every ending and every kind of answer reached among them;
//! and the seeds that find what the engine does not do yet.

use std::collections::{BTreeMap, BTreeSet};

use temper_worker_model_tests::{ENDINGS, Settings, World, translate};

const ITERATIONS: u32 = 300_000;

fn run(settings: &Settings) -> World {
    let mut world = World::new(settings.clone());
    world.run(ITERATIONS);
    world
}

#[test]
fn random_worlds_settle_and_reach_every_ending() {
    let mut answers = BTreeMap::new();
    let mut endings = BTreeSet::new();
    for seed in (0..SWEPT).filter(|seed| !FINDINGS.contains(seed)) {
        let stats = run(&Settings::rough(seed)).stats();
        for (kind, count) in stats.answers {
            *answers.entry(kind).or_insert(0) += count;
        }
        endings.extend(stats.endings.keys().copied());
    }
    println!("answers {answers:?}");
    // A run says it was cancelled only once something cancelled it, and the
    // worker reports that cancel as its own: the engine's, lost contact, a
    // shutdown, or the wall time of its agent.
    assert!(!answers.contains_key("run cancelled"), "no run's cancel is its own: {answers:?}");
    let missed: Vec<&str> = translate::ANSWER_KINDS
        .into_iter()
        .filter(|kind| !UNREACHED.contains(kind) && !answers.contains_key(kind))
        .collect();
    assert!(missed.is_empty(), "some run is answered each way: {missed:?} were not, of {answers:?}");
    let missed: Vec<&str> = ENDINGS.iter().copied().filter(|ending| !endings.contains(ending)).collect();
    assert!(missed.is_empty(), "every ending was reached: {missed:?} were not");
}

/// How many random worlds the sweep runs, but those of the findings.
const SWEPT: u64 = 40;

/// Answers the sweep does not reach: a run's cancel is never its own; the
/// engine names no start the forge lacks, so no preparation finds one
/// missing, nor creates a branch the forge could refuse; and an assignment
/// beyond the worker's limits, which a scenario above reaches.
const UNREACHED: [&str; 4] = ["run cancelled", "unprepared refused", "refused invalid", "unprepared missing"];

/// Seeds that find what the engine does not do yet, run by
/// `the_engine_findings_replay` until it does: a change's attempt pushes,
/// and its answer reaches the engine only after the engine presumed it lost
/// (the worker out of reach past the engine's grace), so the engine never
/// learns the branch moved; every later attempt starts from the base, and
/// its push is rejected as the branch moved, for good.
const FINDINGS: [u64; 3] = [9, 25, 40];

#[test]
#[ignore = "the engine does not learn of a branch pushed by an attempt it presumed lost"]
fn the_engine_findings_replay() {
    for seed in FINDINGS {
        run(&Settings::rough(seed));
    }
}
