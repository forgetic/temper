//! The engine's world, story by story, then all at once.

use temper_engine_model::{Item, Limits, plan};
use temper_engine_model_tests::people::Story;
use temper_engine_model_tests::referee::Moment;
use temper_engine_model_tests::{Settings, World, deployment};
use temper_lib::Duration;
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

/// The story `story` alone, its engine restarting at `moment`, before it
/// hears that what it asked was done.
fn restarted_at(seed: u64, story: Story, moment: Moment) -> World {
    let world = run(Settings { restart_on: vec![moment], ..Settings::only(seed, &[story]) });
    assert_eq!(world.stats().restarts, 1, "the engine restarted at {moment:?}");
    assert!(closed(&world, 0), "the story is done");
    world
}

#[test]
fn a_restart_between_an_outcomes_comment_and_its_record_applies_it_once() {
    let world = restarted_at(11, Story::Hello, Moment::Outcome);
    assert_eq!(world.again(), 0, "nothing is made twice");
}

#[test]
fn a_restart_between_a_plans_writes_makes_the_rest_once() {
    let world = restarted_at(12, Story::Plan, Moment::PlanItem);
    assert_eq!(world.again(), 0, "nothing is made twice");
}

#[test]
fn a_restart_between_a_growths_two_records_grows_the_plan_once() {
    let world = restarted_at(13, Story::Plan, Moment::Growth);
    assert_eq!(world.again(), 0, "nothing is made twice");
}

#[test]
fn a_restart_between_a_claims_write_and_its_assignment_runs_it_once() {
    let world = restarted_at(14, Story::Fix, Moment::Claim);
    assert_eq!(world.again(), 0, "nothing is made twice");
}

#[test]
fn a_session_whose_tracking_label_a_person_took_off_is_found_after_a_restart() {
    restarted_at(15, Story::Unlabel, Moment::Unlabelled);
}

#[test]
fn a_record_a_person_garbled_holds_its_item_for_them() {
    let world = restarted_at(16, Story::Mangle, Moment::Mangled);
    assert!(world.stats().endings.contains_key("mangled"), "the record read is held as mangled: {:?}", world.stats());
}

#[test]
fn a_person_watches_a_session_and_stops_its_run() {
    let world = run(Settings::only(17, &[Story::Stop]));
    assert!(closed(&world, 0), "the session is done");
    let people = world.stats().people;
    assert!(people.watches == 1 && people.stops == 1, "the person watched it, and stopped its run: {people:?}");
    assert!(people.releases >= 1, "the run stopped, the item was held and released: {people:?}");
}

#[test]
fn a_supervisor_whose_wake_batches_is_woken_once_by_a_burst_of_its_steps_ending() {
    let batch = plan::Batch { count: 2, age: Some(Duration::from_secs(300)) };
    let world = run(Settings { batch, ..Settings::only(18, &[Story::Burst]) });
    assert!(closed(&world, 0), "the goal's session is done");
    let session = world.item(0).expect("the session's item is known");
    let spikes: Vec<Item> = world
        .mirror()
        .items()
        .filter_map(|(repository, number, _)| {
            let record = world.mirror().record(repository, number)?;
            let spike = record.relations.goal == Some(session) && record.step.step.name.starts_with(b"spike");
            let repository = deployment::index(repository)?;
            spike.then_some(Item { repository, number })
        })
        .collect();
    assert_eq!(spikes.len(), 2, "the plan's two spikes");
    let ends: Vec<_> = world.closings().iter().filter(|(_, item)| spikes.contains(item)).map(|(at, _)| *at).collect();
    let (first, last) = (ends[0], ends[1]);
    let between = world.assignments().iter().filter(|(at, item, _)| *item == session && *at > first && *at < last);
    assert_eq!(between.count(), 0, "the session is not woken by one spike's end alone");
    let after = world.assignments().iter().filter(|(at, item, _)| *item == session && *at >= last);
    assert!(after.count() >= 1, "the burst wakes it");
}

#[test]
fn an_approval_of_an_earlier_head_lands_nothing() {
    let world = run(Settings { dismiss_stale: false, ..Settings::only(19, &[Story::Stale]) });
    assert!(closed(&world, 0), "the session is done");
    let person = deployment::PEOPLE[0];
    let landed: Vec<_> = world
        .mirror()
        .items()
        .filter_map(|(_, _, issue)| Some((issue.pull.as_ref()?.commit, issue.pull.as_ref()?.merged?, &issue.reviews)))
        .collect();
    assert_eq!(landed.len(), 1, "the change landed");
    let (head, _, reviews) = landed[0];
    let stale = reviews.iter().any(|review| review.by == person && review.commit != head);
    assert!(stale, "its person approved an earlier head: {reviews:?}");
}
