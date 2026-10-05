#[test]
fn seed_replays_with_and_without_fact_consumption() {
    assert_eq!(
        temper_engine_tasks_world::run_story_facts(23, true),
        temper_engine_tasks_world::run_story_facts(23, false)
    );
    temper_world::assert_replays(23, 24, temper_engine_tasks_world::run_story);
}
