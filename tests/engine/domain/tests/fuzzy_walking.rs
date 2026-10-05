use skein_lib::Rng;
use std::collections::BTreeSet;
use temper_engine_domain_world::walking::{Settings, run_replayed};

#[test]
fn fuzzy_walking_commit_cuts_latency_replay_and_observation_pressure() {
    let mut covered = BTreeSet::new();
    for seed in 0..64 {
        let mut random = Rng::new(seed + 7100);
        let settings = Settings {
            seed,
            restart: random.below(2) == 0,
            commit_delay: u32::try_from(random.below(3)).expect("tiny latency"),
            page_delay: u32::try_from(random.below(3)).expect("tiny latency"),
            facts: random.below(2) == 0,
        };
        let world = run_replayed(settings);
        assert!(world.referee.done(), "seed {seed}");
        assert_eq!(world.restarts, u32::from(settings.restart), "seed {seed}");
        covered.insert((settings.restart, settings.commit_delay != 0, settings.page_delay != 0));
    }
    assert_eq!(covered.len(), 8, "both commit cuts across delayed/immediate store and page terminals");
}
