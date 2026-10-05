use temper_engine_domain_world::commits::random;

#[test]
fn bounded_random_decisions_settle_under_store_and_ready_lag() {
    for seed in 0_u64..64 {
        let world = random(seed);
        assert!(world.referee.done(), "seed {seed}");
    }
}
