use temper_engine_domain_forge as top;
use temper_engine_domain_forge_issues as issues;
use temper_engine_forge_world::{REPO, World};

fn run(seed: u64, facts: u32) -> Box<[top::Request]> {
    let mut world = World::with_facts(seed, facts);
    world.adopt();
    world.event(top::Event::Project {
        entry: 1,
        repository: REPO,
        view: issues::GoalView {
            phase: issues::Phase::Waiting,
            goal: 42,
            repository: u64::from(REPO.repository),
            title: Box::from("Goal"),
            goal_text: Box::from("Plan"),
            plan: Box::new([]),
            milestones: Box::new([]),
            finished: None,
        },
    });
    world.event(top::Event::Committed { entry: 1 });
    world.run_for(1);
    world.restart();
    world.run_for(20);
    assert_eq!(world.writes(), 1);
    assert!(world.stored().get(&top::Key::Entry(1)).is_none());
    world.take_seen()
}

#[test]
fn a_seed_replays_after_restart_and_fact_capacity_changes_no_decision() {
    let first = run(41, 16);
    assert_eq!(first, run(41, 16));
    assert_eq!(first, run(41, 0));
}
