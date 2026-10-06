//! Small bounded role-reroute sweep with all genuine semantic starting states,
//! both owner orders, durable cuts and deterministic delayed-store replay
//! (testing-strategy.md, section 8).

use std::collections::BTreeSet;
use temper_engine_domain_world::roles::{Base, Settings, reroute_replayed};

#[test]
fn twelve_small_role_histories_cover_each_holder_owner_order_and_atomic_cut() {
    let bases = [Base::Requester, Base::FinalRole, Base::Rejected];
    let mut covered = BTreeSet::new();
    for seed in 0..12_u64 {
        let base = usize::try_from(seed % 3).expect("three actual semantic states");
        let winner = usize::try_from((seed / 3) % 2).expect("two authenticated owners");
        let cut = seed >= 6;
        let settings = Settings {
            cut,
            commit_delay: u32::try_from(seed % 3).expect("tiny store delay"),
            page_delay: u32::try_from((seed / 3) % 2).expect("tiny page delay"),
            facts: seed.is_multiple_of(2),
            ..Settings::calm(9400 + seed, bases[base])
        };
        let world = reroute_replayed(settings, winner);
        assert!(world.referee.done(), "seed {seed}");
        assert_eq!(world.referee.replacements, 1, "seed {seed}");
        assert_eq!(world.restarts, u32::from(cut), "seed {seed}");
        covered.insert((base, winner, cut));
    }
    assert_eq!(covered.len(), 12, "every actual holder/owner/cut cell ran");
}
