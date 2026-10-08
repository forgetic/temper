//! Deterministic whole-worker boundary replay against the forge and disk.
use temper_worker_domain_world::whole::{Case, run};

#[test]
fn seeded_worker_delivery_and_contact_stories_replay() {
    for seed in 0..16 {
        for case in [Case::Delivered, Case::CancelledDelivery, Case::ContactWithin, Case::ContactPast] {
            assert_eq!(run(seed, case), run(seed, case), "seed {seed}, case {case:?}");
        }
    }
}
