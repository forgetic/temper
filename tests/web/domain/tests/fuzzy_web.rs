use skein_lib::Duration;
use temper_fake_person::RandomPerson;
use temper_web_domain_world::{Fault, Scenario, Settings, World};

#[test]
fn seeded_people_and_faults_keep_the_world_consistent() {
    // Seeds retained after link, watch-terminal and same-bytes draft races.
    const PINNED: [u64; 3] = [5, 13, 16];
    let mut coverage = std::collections::BTreeMap::<Fault, u64>::new();
    let mut made = 0_u64;
    for seed in PINNED.into_iter().chain((0..32).filter(|seed| !PINNED.contains(seed))) {
        let mut settings = Settings::random(seed);
        settings.restart_per_mille = 10;
        settings.drop_per_mille = 15;
        settings.miss_per_mille = 15;
        settings.reload_per_mille = 10;
        settings.signed_out_per_mille = 10;
        let mut world = World::new(settings, Scenario::default());
        let mut person = RandomPerson::new(seed ^ 0x8235);
        world.advance(Duration::from_millis(20));
        for _ in 0..150 {
            world.random_step(&mut person);
        }
        world.settle();
        made += world.engine.creations;
        for (fault, count) in world.stats.faults {
            *coverage.entry(fault).or_default() += count;
        }
    }
    assert!(made > 0, "random persons start a chat");
    for fault in [
        Fault::Restart,
        Fault::Busy,
        Fault::Dropped,
        Fault::Missed,
        Fault::Reload,
        Fault::DoublePress,
        Fault::SignedOut,
        Fault::Latency,
    ] {
        assert!(coverage.get(&fault).copied().unwrap_or(0) > 0, "fault {fault:?} reached");
    }
}
