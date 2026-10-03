//! The worker's checkout child domain at random: many noisy worlds, each
//! settled with every client heard, every way a client ends reached among them.

use std::collections::BTreeSet;

use temper_worker_domain_checkout::{Failure, Landing, Prepared};
use temper_worker_domain_checkout_tests::{World, noisy, submit_noisily};

const ITERATIONS: u32 = 100_000;

/// A thousand worlds with random limits, faults, schedules and clients: each
/// settles, with every client's every request ended once and nothing left
/// held or in flight (checked by `World::run`), and between them they reach
/// every way a prepare, a push and a save can end, and every way the cache
/// can find a workspace.
#[test]
fn random_worlds_settle_with_every_client_heard() {
    let mut prepares = BTreeSet::new();
    let mut lands = BTreeSet::new();
    let (mut cached, mut races) = ([0; 4], [0; 9]);
    for seed in 0..1000 {
        let settings = noisy(seed);
        let mut world = World::new(settings);
        submit_noisily(&mut world, &settings, seed);
        world.run(ITERATIONS);
        let (told, stats) = (world.told().0, world.stats());
        for (count, more) in cached.iter_mut().zip([told.new, told.reused, told.rebuilt, told.evicted]) {
            *count += more;
        }
        let won = [
            stats.cancels_lost,
            stats.cancels_crossed,
            stats.op_timeouts,
            stats.stale,
            stats.exists,
            stats.op_broken,
            stats.ambiguous,
            stats.verified,
            stats.made_over,
        ];
        for (count, more) in races.iter_mut().zip(won) {
            *count += more;
        }
        for (_, client) in world.clients() {
            let kind = match client.prepared.expect("every prepare ends") {
                Prepared::Ready { .. } => "ready".to_string(),
                Prepared::Refused { refusal } => format!("{refusal:?}"),
                Prepared::Failed { failure: Failure::Missing { missing, .. } } => format!("missing {missing:?}"),
                Prepared::Failed { failure } => format!("{failure:?}"),
                Prepared::Aborted => "aborted".to_string(),
            };
            prepares.insert(kind);
            for (saved, landings) in &client.landings {
                for landing in landings {
                    let kind = match landing {
                        Landing::Landed { .. } => "landed",
                        Landing::Moved => "moved",
                        Landing::Failed => "failed",
                        Landing::Refused => "refused",
                        Landing::Unchanged => "unchanged",
                        Landing::Aborted => "aborted",
                    };
                    lands.insert(format!("{} {kind}", if *saved { "save" } else { "push" }));
                }
            }
        }
    }
    let expected = [
        "ready",
        "Busy",
        "Full",
        "Invalid",
        "Transient",
        "Refused { repository: 0 }",
        "missing Repository",
        "missing Branch",
        "missing Commit",
        "aborted",
    ];
    for kind in expected {
        assert!(prepares.contains(kind), "some prepare ended {kind}: {prepares:?}");
    }
    for verb in ["push", "save"] {
        for kind in ["landed", "moved", "failed", "refused", "unchanged", "aborted"] {
            let kind = format!("{verb} {kind}");
            assert!(lands.contains(&kind), "some repository's {kind}: {lands:?}");
        }
    }
    assert!(cached.iter().all(|&count| count > 0), "workspaces new, reused, rebuilt and evicted: {cached:?}");
    assert!(
        races.iter().all(|&count| count > 0),
        "cancels lost and crossed, deadlines passed, stale handles, base branches created meanwhile, io failed, \
         io in doubt, pushes verified, workspaces made over something: {races:?}"
    );
}
