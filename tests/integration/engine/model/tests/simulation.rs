//! The engine's world, story by story, then all at once.

use temper_engine_model::Limits;
use temper_engine_model_tests::people::Story;
use temper_engine_model_tests::{Settings, World};
use temper_world::assert_replays;

const ITERATIONS: u32 = 200_000;

fn run(settings: Settings) -> World {
    let mut world = World::new(settings);
    world.run(ITERATIONS);
    world
}

/// Whether the item of the story `tale` is closed.
fn closed(world: &World, tale: usize) -> bool {
    let item = world.item(tale).expect("the story's item is known");
    let name = temper_engine_model_tests::deployment::name(item.repository);
    !world.mirror().issue(name, item.number).expect("on the forge").open
}

#[test]
fn a_session_from_the_web_says_hello_and_finishes() {
    let world = run(Settings::only(1, &[Story::Hello]));
    assert!(closed(&world, 0), "the session is done");
}

#[test]
fn an_issue_handed_in_is_fixed_through_a_red_change_repaired_reviewed_and_landed() {
    let world = run(Settings::only(2, &[Story::Fix]));
    assert!(closed(&world, 0), "the session is done");
    let stats = world.stats();
    let pushes: u32 = stats.workers.iter().map(|worker| worker.pushes).sum();
    assert!(pushes >= 2, "the change was made, then repaired: {stats:?}");
    assert!(stats.people.reviews >= 1, "a person reviewed it: {stats:?}");
    let merged =
        world.mirror().items().filter(|(_, _, issue)| issue.pull.as_ref().is_some_and(|pull| pull.merged.is_some()));
    assert_eq!(merged.count(), 1, "the change landed");
}

#[test]
fn a_session_chats_parks_and_is_resumed_from_its_snapshot() {
    let world = run(Settings::only(3, &[Story::Chat]));
    assert!(closed(&world, 0), "the session is done");
    let stats = world.stats();
    let parked: u32 = stats.workers.iter().map(|worker| worker.parked).sum();
    assert_eq!(parked, 1, "the session parked: {stats:?}");
    assert!(stats.store.found >= 1, "its snapshot was read back to resume it: {stats:?}");
}

#[test]
fn a_note_written_by_a_run_and_corrected_by_a_person_is_recalled_as_corrected() {
    let world = run(Settings::only(4, &[Story::Notes]));
    assert!(closed(&world, 0), "the session is done");
}

#[test]
fn a_plan_is_proposed_accepted_decided_grown_and_landed() {
    let world = run(Settings::only(6, &[Story::Plan]));
    assert!(closed(&world, 0), "the goal's session is done");
    let merged =
        world.mirror().items().filter(|(_, _, issue)| issue.pull.as_ref().is_some_and(|pull| pull.merged.is_some()));
    assert_eq!(merged.count(), 3, "the design and the two changes the build added landed");
}

#[test]
fn growth_beyond_the_envelope_waits_for_acceptance() {
    let world = run(Settings::only(7, &[Story::Grow]));
    assert!(closed(&world, 0), "the goal's session is done");
}

#[test]
fn a_rejected_proposal_is_dropped() {
    let world = run(Settings::only(8, &[Story::Reject]));
    assert!(closed(&world, 0), "the session is done");
    assert_eq!(world.mirror().items().count(), 1, "nothing of the plan was made");
}

#[test]
fn a_change_whose_ci_never_reports_stalls_and_is_held() {
    let world = run(Settings::only(9, &[Story::Stall]));
    assert!(closed(&world, 0), "its person closed the session");
    assert!(world.stats().people.releases >= 1, "the stalled change was released once at least");
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
        let none = run(Settings { limits: Limits { facts: 0, ..settings.limits }, ..settings.clone() });
        let many = run(Settings { limits: Limits { facts: 4_096, ..settings.limits }, ..settings });
        assert!(none.trace() == many.trace(), "seed {seed}: the same run whatever facts are kept");
    }
}

/// Every story at once: plans' changes land on one branch beside others,
/// each rebased as the base moves under it.
#[test]
fn every_story_at_once_settles() {
    let world = run(Settings::calm(5));
    for tale in 0..world.stories() {
        assert!(closed(&world, tale), "story {tale} is done");
    }
}
