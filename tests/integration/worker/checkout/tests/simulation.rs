//! End to end at the checkout child domain: the checkout, its scripted clients,
//! a fake forge and a fake disk, talking through a simulated world.

use temper_lib::{Duration, Time};
use temper_worker_domain_checkout::git::Missing;
use temper_worker_domain_checkout::{Failure, Landing, Limits, Prepared, Refusal};
use temper_worker_domain_checkout_tests::client::{Interrupt, Pick, Plan};
use temper_worker_domain_checkout_tests::translate;
use temper_worker_domain_checkout_tests::{LIMITS, Settings, Span, Told, World, noisy, submit_noisily};

const ITERATIONS: u32 = 100_000;

fn at(secs: u64) -> Time {
    Time::ZERO.saturating_add(Duration::from_secs(secs))
}

fn prepared(world: &World, client: u64) -> Prepared {
    world.client(client).prepared.expect("every client's prepare ends")
}

/// The landings of a client's pushes and saves, in order.
fn landings(world: &World, client: u64) -> Vec<(bool, Vec<Landing>)> {
    world.client(client).landings.iter().map(|(saved, landings)| (*saved, landings.to_vec())).collect()
}

/// What a client's first repository's pushes came to.
fn first_landings(world: &World, client: u64) -> Vec<Landing> {
    landings(world, client).into_iter().map(|(_, landings)| landings[0]).collect()
}

#[test]
fn a_client_prepares_edits_pushes_and_its_change_lands_on_a_base_branch_created_for_it() {
    let mut world = World::new(Settings::calm(1));
    let client = world.submit(Time::ZERO, Plan::simple(0));
    world.run(ITERATIONS);
    assert!(matches!(prepared(&world, client), Prepared::Ready { .. }));
    let [Landing::Landed { commit }] = first_landings(&world, client)[..] else {
        panic!("the change landed: {:?}", landings(&world, client));
    };
    let repository = world.repository(world.workstream(0)[0]);
    assert_eq!(world.forge().branch(repository, b"base/0"), Some(translate::fake(commit)));
    let stats = world.stats();
    assert!(stats.created >= 1, "the base branch was created");
    assert_eq!((stats.releases, stats.moved), (1, 0));
}

#[test]
fn a_workstream_reuses_its_workspace_and_others_evict_the_least_recently_used() {
    let settings = Settings { checkout: Limits { workspaces: 2, ..LIMITS }, ..Settings::calm(2) };
    let mut world = World::new(settings);
    for (n, workstream) in [0, 1, 0, 2, 1, 3].into_iter().enumerate() {
        world.submit(at(100 * u64::try_from(n).expect("small")), Plan::simple(workstream));
    }
    world.run(ITERATIONS);
    let (told, lost) = world.told();
    assert_eq!(lost, 0);
    // 0 and 1 are new; 0 is reused; 2 evicts 1, the least recently used; 1
    // evicts 0; 3 evicts 2.
    assert_eq!((told.new, told.reused, told.evicted), (2, 1, 3), "{told:?}");
    assert_eq!(world.stats().most_workspaces, 2, "the cache stays within its bound");
}

#[test]
fn a_held_workstream_is_busy_and_a_full_cache_refuses() {
    let settings = Settings { checkout: Limits { workspaces: 1, ..LIMITS }, ..Settings::calm(3) };
    let mut world = World::new(settings);
    let first = world.submit(Time::ZERO, Plan::simple(0));
    let same = world.submit(Time::ZERO, Plan::simple(0));
    let other = world.submit(Time::ZERO, Plan::simple(1));
    world.run(ITERATIONS);
    assert!(matches!(prepared(&world, first), Prepared::Ready { .. }));
    assert_eq!(prepared(&world, same), Prepared::Refused { refusal: Refusal::Busy });
    assert_eq!(prepared(&world, other), Prepared::Refused { refusal: Refusal::Full });
}

#[test]
fn a_spec_beyond_the_limits_is_refused() {
    let mut world = World::new(Settings::calm(4));
    let client = world.submit(Time::ZERO, Plan { invalid: true, ..Plan::simple(0) });
    world.run(ITERATIONS);
    assert_eq!(prepared(&world, client), Prepared::Refused { refusal: Refusal::Invalid });
}

#[test]
fn a_spec_naming_what_the_forge_lacks_fails_for_good() {
    let cases = [
        (Pick::MissingRepository, Missing::Repository),
        (Pick::MissingBranch, Missing::Branch),
        (Pick::MissingCommit, Missing::Commit),
    ];
    for (pick, missing) in cases {
        let mut world = World::new(Settings::calm(5));
        let client = world.submit(Time::ZERO, Plan { start: Some(pick), ..Plan::simple(0) });
        world.run(ITERATIONS);
        let Prepared::Failed { failure: Failure::Missing { repository: 0, missing: found } } = prepared(&world, client)
        else {
            panic!("{pick:?} is missing: {:?}", prepared(&world, client));
        };
        assert_eq!(found, missing);
        assert_eq!(world.stats().releases, 1, "the client released its hold");
    }
}

#[test]
fn an_unreachable_forge_fails_the_prepare_for_now_and_a_refusing_one_for_good() {
    let mut world = World::new(Settings { unreachable: 1000, ..Settings::calm(6) });
    let client = world.submit(Time::ZERO, Plan::simple(0));
    world.run(ITERATIONS);
    assert_eq!(prepared(&world, client), Prepared::Failed { failure: Failure::Transient });
    // A base branch the forge refuses to create.
    let mut world = World::new(Settings { refusing: 1000, ..Settings::calm(6) });
    let client = world.submit(Time::ZERO, Plan::simple(0));
    world.run(ITERATIONS);
    assert_eq!(prepared(&world, client), Prepared::Failed { failure: Failure::Refused { repository: 0 } });
    // A push the forge refuses, from a branch it does not create.
    let mut world = World::new(Settings { refusing: 1000, ..Settings::calm(6) });
    let client = world.submit(Time::ZERO, Plan { start: Some(Pick::Branch), ..Plan::simple(0) });
    world.run(ITERATIONS);
    assert_eq!(first_landings(&world, client), [Landing::Refused]);
}

#[test]
fn a_branch_another_party_advanced_makes_the_push_moved_and_nothing_is_forced() {
    let mut moved = 0;
    for seed in 0..20 {
        let mut world = World::new(Settings { advance: 1000, ..Settings::calm(seed) });
        let client = world.submit(Time::ZERO, Plan { pushes: 2, ..Plan::simple(0) });
        world.run(ITERATIONS);
        for landing in first_landings(&world, client) {
            match landing {
                Landing::Landed { .. } => {}
                Landing::Moved => moved += 1,
                other @ (Landing::Failed | Landing::Refused | Landing::Unchanged | Landing::Aborted) => {
                    panic!("seed {seed}: landed or moved, not {other:?}")
                }
            }
        }
    }
    assert!(moved > 0, "some pushes found their branch moved");
}

#[test]
fn saved_work_is_where_a_later_client_resumes() {
    let mut world = World::new(Settings::calm(7));
    let saver = world.submit(Time::ZERO, Plan { pushes: 0, save: true, ..Plan::simple(0) });
    let resumer = world.submit(at(100), Plan { start: Some(Pick::Saved), pushes: 0, ..Plan::simple(0) });
    world.run(ITERATIONS);
    let [(true, ref work)] = landings(&world, saver)[..] else { panic!("the first client saved") };
    assert!(matches!(work[0], Landing::Landed { .. }), "its work was saved: {work:?}");
    let left = &world.client(saver).left[0];
    assert_eq!(&world.client(resumer).start[0], left, "the second starts where the first left off");
    assert_ne!(left, &world.client(saver).start[0], "and that is not where the first started");
}

#[test]
fn an_abort_ends_the_prepare_under_way_and_what_it_built_is_built_again() {
    let mut rebuilt = 0;
    for seed in 0..20 {
        let interrupt = Some((Duration::from_millis(30), Interrupt::Abort));
        let mut world = World::new(Settings::calm(seed));
        let aborted = world.submit(Time::ZERO, Plan { interrupt, ..Plan::simple(0) });
        let later = world.submit(at(100), Plan::simple(0));
        world.run(ITERATIONS);
        assert_eq!(prepared(&world, aborted), Prepared::Aborted, "seed {seed}");
        assert!(world.client(aborted).landings.is_empty(), "seed {seed}: an aborted prepare pushes nothing");
        assert!(matches!(prepared(&world, later), Prepared::Ready { .. }), "seed {seed}");
        rebuilt += world.told().0.rebuilt;
    }
    assert!(rebuilt > 0, "some aborts cut a build short, which the next prepare made again");
}

#[test]
fn a_release_under_way_aborts_first_then_releases() {
    let mut aborted = 0;
    for seed in 0..40 {
        let interrupt = Some((Duration::from_millis(1_200), Interrupt::Release));
        let mut world = World::new(Settings { think: Span::millis(0, 50), ..Settings::calm(seed) });
        let client = world.submit(Time::ZERO, Plan { pushes: 3, interrupt, ..Plan::simple(0) });
        world.run(ITERATIONS);
        aborted += world.stats().landings_aborted + world.stats().prepares_aborted;
        assert_eq!(world.stats().releases, 1, "seed {seed}: released once");
        assert!(world.client(client).landings.len() <= 3, "seed {seed}");
    }
    assert!(aborted > 0, "some releases came while something was under way");
}

#[test]
fn a_push_sent_twice_is_refused_as_busy_and_the_first_goes_on() {
    let mut world = World::new(Settings::calm(8));
    let client = world.submit(Time::ZERO, Plan { twice: true, ..Plan::simple(0) });
    world.run(ITERATIONS);
    assert_eq!(world.stats().busy_pushes, 1);
    assert!(matches!(first_landings(&world, client)[..], [Landing::Landed { .. }]));
}

#[test]
fn what_an_earlier_worker_left_does_not_fail_a_prepare() {
    let mut world = World::new(Settings { leftovers: 1000, ..Settings::calm(9) });
    let client = world.submit(Time::ZERO, Plan::simple(0));
    world.run(ITERATIONS);
    assert!(matches!(prepared(&world, client), Prepared::Ready { .. }));
    assert_eq!(world.stats().made_over, 1, "the new workspace was made over what was left there");
}

#[test]
fn a_push_io_reports_run_out_of_time_is_verified_and_reported_as_it_went() {
    let mut verified = 0;
    for seed in 0..40 {
        let mut world = World::new(Settings { ambiguous: 200, ..Settings::calm(seed) });
        world.submit(Time::ZERO, Plan { pushes: 3, ..Plan::simple(0) });
        world.run(ITERATIONS);
        verified += world.stats().verified;
    }
    assert!(verified > 0, "some pushes io reported run out of time had landed, and were reported landed");
}

#[test]
fn a_seed_replays_to_the_same_run() {
    let trace = temper_world::assert_replays(13, 14, |seed| {
        let settings = noisy(seed);
        let mut world = World::new(settings);
        submit_noisily(&mut world, &settings, seed);
        world.run(ITERATIONS);
        (world.trace().to_vec(), (world.stats(), world.now()))
    });
    assert!(trace.len() > 20, "the run did something");
}

/// Facts are told on the side: a checkout that keeps none of them makes the
/// same requests at the same times as one that keeps them all, and the facts
/// kept add up to what crossed the boundary (checked by `World::run`).
#[test]
fn facts_change_nothing_the_checkout_does() {
    for seed in 0..100 {
        let run = |facts| {
            let noisy = noisy(seed);
            let settings = Settings { checkout: Limits { facts, ..noisy.checkout }, ..noisy };
            let mut world = World::new(settings);
            submit_noisily(&mut world, &settings, seed);
            world.run(ITERATIONS);
            (world.trace().to_vec(), world.stats(), world.told())
        };
        let (trace, stats, (told, lost)) = run(4096);
        assert_eq!(lost, 0, "seed {seed}: room for every fact");
        assert!(told.started > 0, "seed {seed}: facts were told");
        let (silent, same, (none, dropped)) = run(0);
        assert!(silent == trace && same == stats, "seed {seed}: the same requests, at the same times");
        assert_eq!((none, dropped > 0), (Told::default(), true), "seed {seed}: every fact dropped");
    }
}
