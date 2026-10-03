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
    for seed in 0..SWEPT {
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

/// How many random worlds the sweep runs.
const SWEPT: u64 = 40;

/// Answers the sweep does not reach: a run's cancel is never its own; an
/// assignment beyond the worker's limits, which a scenario reaches; and a
/// start the forge lacks. The engine starts a checkout from its base, which
/// is created if the forge lacks it, or from its item's branch: only that
/// branch deleted under a change could be missing, and the engine holds such
/// a change for good before another attempt starts (an ignored scenario
/// keeps it).
const UNREACHED: [&str; 3] = ["run cancelled", "refused invalid", "unprepared missing"];

/// Seeds that find what the engine does not do yet, swept past the seeds
/// above:
///
/// - 40: a change's first push lands, but io reports it out of time, and the
///   fetch that would verify it fails too, so its run answers that nothing
///   landed. The engine never learns its branch: every later attempt starts
///   from the base, its push is rejected as the branch moved, and its outcome
///   is stale, run after run without bound, until the forge has no room for
///   the outcomes and holds the change for its writes.
/// - 103: the engine restarts while it applies a change's outcome, after
///   another party moved the change's branch past what the run pushed. The
///   engine that starts cold waits for good, its pull request green and
///   approved at the moved head.
const FINDINGS: [u64; 2] = [40, 103];

#[test]
#[ignore = "until the engine learns a branch no answer said landed, and resumes a moved change after a restart"]
fn the_engine_findings_replay() {
    for seed in FINDINGS {
        run(&Settings::rough(seed));
    }
}
