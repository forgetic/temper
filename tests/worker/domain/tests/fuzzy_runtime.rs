//! Small retained-turn windows and body budgets; deterministic seeds.
use temper_worker_domain_world::next::{Settings, World};

#[test]
fn v2_runtime_random_turn_sizes_and_tiny_windows_settle() {
    for seed in 0..48 {
        let turns = u32::try_from(seed % 3 + 1).expect("tiny window");
        let byte_slots = u32::try_from(seed / 3 % 3 + 1).expect("tiny budget");
        let settings = Settings { seed, turns, byte_slots, merging: seed % 2 == 0, abandon: seed % 5 == 0 };
        let result = std::panic::catch_unwind(|| {
            let mut world = World::new(settings);
            world.run();
            // Stopping also takes the independent agent window already sent.
            assert!(world.stats().peak_retained <= turns * 2);
        });
        assert!(result.is_ok(), "v2 runtime seed {seed}, settings {settings:?}");
    }
}
