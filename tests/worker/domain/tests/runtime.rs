//! Focused v2 runtime stories and full trace replay.
use temper_worker_domain_world::next::{Settings, World};

fn run(settings: Settings) -> World {
    let mut world = World::new(settings);
    world.run();
    world
}

#[test]
fn fenced_turns_survive_busy_reconnect_and_crossed_acknowledgements() {
    let world = run(Settings { seed: 5, turns: 3, byte_slots: 2, merging: false, abandon: false });
    let stats = world.stats();
    assert!(stats.copies > stats.turns);
    assert_eq!(stats.busy, stats.turns);
    assert!(stats.reconnects > stats.turns);
    assert!(stats.peak_retained <= 6);
    assert_eq!(stats.saves, 1);
}

#[test]
fn merge_paths_and_expected_heads_cross_the_worker_and_park_has_no_saved_merge() {
    let world = run(Settings { seed: 19, turns: 2, byte_slots: 1, merging: true, abandon: false });
    assert_eq!(world.stats().commits, 2, "unresolved paths then an explicit resolution");
    assert_eq!(world.stats().saves, 0);
}

#[test]
fn contact_grace_abandons_retained_turns_and_the_slot_together() {
    let world = run(Settings { seed: 7, turns: 3, byte_slots: 3, merging: false, abandon: true });
    assert!(world.stats().abandoned > 0);
}

#[test]
fn v2_full_boundary_and_fact_traces_replay_exactly() {
    let settings = Settings { seed: 27, turns: 2, byte_slots: 2, merging: true, abandon: false };
    let first = run(settings);
    let second = run(settings);
    assert_eq!(first.stats(), second.stats());
    assert_eq!(first.trace(), second.trace());
}

#[test]
fn v2_facts_can_be_dropped_without_changing_the_runtime_boundaries() {
    let settings = Settings { seed: 13, turns: 3, byte_slots: 2, merging: false, abandon: false };
    let first = run(settings);
    let mut second = World::without_facts(settings);
    second.run();
    assert_eq!(first.boundaries(), second.boundaries());
}
