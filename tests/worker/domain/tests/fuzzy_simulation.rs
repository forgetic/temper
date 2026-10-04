//! The whole worker's world at random: many rough worlds against the engine,
//! each settled, every ending and every kind of answer reached among them;
//! and the seeds that find what the engine does not do yet.

use std::collections::{BTreeMap, BTreeSet};

use temper_worker_domain_world::{ENDINGS, Settings, World, translate};

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
    for seed in (0..SWEPT).chain(PINNED) {
        let stats = run(&Settings::rough(seed)).stats();
        for (kind, count) in stats.answers {
            *answers.entry(kind).or_insert(0) += count;
        }
        endings.extend(stats.endings.keys().copied());
    }
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

/// How many random worlds the sweep runs: as many as take about six
/// seconds on their own, within the fuzzy suite's budget.
const SWEPT: u64 = 40;

/// Answers the sweep does not reach: a run's cancel is never its own; and
/// an assignment beyond the worker's limits, which a scenario reaches.
const UNREACHED: [&str; 2] = ["run cancelled", "refused invalid"];

/// Seeds swept besides those above, each for what it once found:
///
/// - 34: asynchronous relay cancellation changes the schedule, and a
///   reopened pull request must refresh its observed head so scripted
///   reviewers stop once they have approved the new commit.
/// - 101: a change's first push lands, but io reports it out of time, and
///   the fetch that would verify it fails too, so its run answers that
///   nothing landed. The engine reads the change's branch on the forge
///   before it makes the change again, and starts from it.
const PINNED: [u64; 2] = [34, 101];

/// Unresolved FINDINGS[0], workflow.md3: replay the intended invariant, rather
/// than treating a namespace collision as correct long-lived recovery.
#[test]
#[ignore = "v1 lacks a cross-restart inbound namespace and worker-to-engine Waiting transport"]
fn finding_12_restarted_sender_never_reuses_an_attempts_event_name_for_different_bytes() {
    let stats = run(&Settings::rough(temper_worker_domain_world::FINDINGS[0])).stats();
    assert_eq!(stats.reused_inbound_names, 0, "one attempt/event name identifies one body across engine restart");
}
