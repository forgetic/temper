use temper_engine_domain_forge as top;
use temper_engine_domain_forge_issues as issues;
use temper_engine_forge_world::{REPO, World};

#[test]
fn seeded_restarts_recover_one_keyed_issue_without_repeating_the_write() {
    for seed in 0..64 {
        let mut world = World::new(seed);
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
        world.run_for(seed % 3);
        world.restart();
        world.run_for(20);
        assert_eq!(world.writes(), 1, "seed {seed}");
        assert!(world.stored().get(&top::Key::Entry(1)).is_none(), "seed {seed}");
    }
}
