use temper_engine_domain_forge_client::{Condition, Effect, Entry, api};
use temper_engine_forge_client_world::{REPO, Settings, World};
#[test]
fn seeded_faults_restarts_and_keyed_effects() {
    let mut unavailable = 0;
    let mut timeouts = 0;
    let mut landings = 0;
    let mut limited = 0;
    for seed in 0..128 {
        let mut world = World::new(Settings::random(seed));
        for entry in 1..=4 {
            world.make(Entry {
                number: entry,
                task: 7,
                repository: REPO,
                effect: Effect {
                    write: api::Write::CreateIssue {
                        key: Box::new([u8::try_from(entry).expect("world entry bound")]),
                        title: Box::from(&b"goal"[..]),
                        body: Box::from(&b"plan"[..]),
                    },
                    condition: Condition::None,
                },
                start: None,
                attempt: None,
                failures: 0,
            });
        }
        world.run_for(2);
        world.restart();
        world.run_for(40);
        world.finish();
        let stats = world.stats();
        assert_eq!(stats.calls, stats.terminals);
        unavailable += stats.unavailable;
        timeouts += stats.timeouts;
        landings += stats.late_landings;
        limited += stats.limited;
    }
    assert!(unavailable > 0 && timeouts > 0 && landings > 0 && limited > 0, "every configured fault actually fell");
}
