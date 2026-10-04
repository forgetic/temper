use temper_engine_protocol_world::link::random;

#[test]
fn authenticated_byte_peers_settle_over_many_seeded_lifecycle_races() {
    let mut stats = [0_u32; 4];
    for seed in 0..128 {
        let (_, result) = random(seed, 64);
        for (total, value) in stats.iter_mut().zip(result) {
            *total += value;
        }
    }
    assert!(stats.iter().all(|&value| value > 50), "admissions/closures/hellos/lost all exercised: {stats:?}");
}
