//! The forge sub-model in its world: scenarios, replay, and a sweep of random
//! worlds.

use temper_engine_model_forge::Limits;
use temper_engine_model_forge_tests::{Settings, Stats, World, parent, people};
use temper_forge_model::{Config, Skew};
use temper_lib::Duration;
use temper_world::assert_replays;

const ITERATIONS: u32 = 2_000_000;

fn run(settings: &Settings) -> World {
    let mut world = World::new(*settings);
    world.run(ITERATIONS);
    world
}

fn count(stats: &Stats, ending: &str) -> u32 {
    stats.endings.get(ending).copied().unwrap_or(0)
}

#[test]
fn a_calm_world_keeps_up_and_settles() {
    let world = run(&Settings::calm(1));
    let stats = world.stats();
    for ending in [
        "announced: missing",
        "offered",
        "news: comment",
        "news: pull",
        "changed",
        "left",
        "loaded",
        "wrote",
        "wrote: created",
        "wrote: commented",
    ] {
        assert!(count(&stats, ending) > 0, "{ending}: {stats:?}");
    }
    for ending in ["timed out", "limited", "full", "wrote: failed"] {
        assert_eq!(count(&stats, ending), 0, "{ending}: {stats:?}");
    }
    let (reached, writes) = world.judged();
    assert!(reached > 10 && writes > 10, "the referee judged: {reached}, {writes}");
}

#[test]
fn every_change_reaches_the_working_set_with_every_webhook_lost() {
    for seed in 0..1 {
        let calm = Settings::calm(seed);
        let settings = Settings { forge: Config { hooks_lost: 1000, ..calm.forge }, ..calm };
        let world = run(&settings);
        let stats = world.stats();
        let forge = stats.forge.expect("counted");
        assert_eq!(forge.hooks, 0, "seed {seed}: no webhook arrived: {forge:?}");
        assert!(world.judged().0 > 0, "seed {seed}: changes reached the working set: {stats:?}");
    }
}

#[test]
fn a_rate_limit_refusal_holds_every_call_until_its_reset() {
    // The engine's own budget is set above the forge's limit, so the forge
    // refuses, and the engine waits for each reset.
    let calm = Settings::calm(3);
    let settings =
        Settings { limits: Limits { rate: 80, ..calm.limits }, forge: Config { rate_limit: 50, ..calm.forge }, ..calm };
    let stats = run(&settings).stats();
    assert!(count(&stats, "limited") > 0, "{stats:?}");
    assert!(stats.forge.expect("counted").limited > 0, "{stats:?}");
}

#[test]
fn restarts_start_cold_and_make_nothing_twice() {
    for seed in 0..1 {
        let calm = Settings::calm(seed);
        let settings = Settings {
            forge: Config { timeouts: 100, ..calm.forge },
            restarts: 4,
            restart_at: temper_engine_model_forge_tests::Span::millis(30_000, 500_000),
            ..calm
        };
        let stats = run(&settings).stats();
        assert_eq!(stats.restarts, 4, "seed {seed}: {stats:?}");
        // A restart may interrupt a cold start; the referee holds each one
        // not interrupted to its bound.
        assert!(count(&stats, "loaded") >= 3, "seed {seed}: a cold start per life: {stats:?}");
        assert!(count(&stats, "announced: found") > 0, "seed {seed}: records found again: {stats:?}");
    }
}

#[test]
fn creations_whose_answers_were_lost_are_found_by_their_keys() {
    let mut found = 0;
    for seed in 0..1 {
        let calm = Settings::calm(seed);
        let settings = Settings {
            forge: Config { timeouts: 200, late: 100, ..calm.forge },
            parent: temper_engine_model_forge_tests::parent::Script { tasks: 500, replies: 600, ..calm.parent },
            ..calm
        };
        let stats = run(&settings).stats();
        assert!(count(&stats, "timed out") > 0, "seed {seed}: {stats:?}");
        found += count(&stats, "found");
    }
    assert!(found > 0, "creations were found by their keys");
}

#[test]
fn a_full_working_set_refuses_new_work_which_waits_on_the_forge() {
    let calm = Settings::calm(4);
    let settings = Settings {
        limits: Limits { items: 3, ..calm.limits },
        parent: temper_engine_model_forge_tests::parent::Script { closes: 300, ..calm.parent },
        ..calm
    };
    let stats = run(&settings).stats();
    assert!(count(&stats, "full") > 0, "{stats:?}");
    assert!(stats.peak <= 3, "{stats:?}");
}

#[test]
fn a_record_a_person_mangled_holds_its_item_until_released() {
    let mut held = 0;
    for seed in 0..1 {
        let calm = Settings::calm(seed);
        let weights = people::Weights { mangles: 6, ..calm.people.weights };
        let settings = Settings { people: people::Script { weights, ..calm.people }, restarts: 1, ..calm };
        let stats = run(&settings).stats();
        held += stats.parent.holds;
    }
    assert!(held > 0, "items were held for a person");
}

#[test]
fn creations_asked_for_again_are_found_after_their_causes_whatever_the_clocks_and_late_landings() {
    let (mut resumed, mut found, mut outcomes, mut landed) = (0, 0, 0, 0);
    for seed in 0..1 {
        let calm = Settings::calm(seed);
        let skew =
            if seed % 2 == 0 { Skew::Behind(Duration::from_secs(40)) } else { Skew::Ahead(Duration::from_secs(40)) };
        let settings = Settings {
            forge: Config { timeouts: 150, landing: 100, skew, ..calm.forge },
            parent: parent::Script { tasks: 500, replies: 600, verdicts: 500, ..calm.parent },
            restarts: 3,
            restart_at: temper_engine_model_forge_tests::Span::millis(30_000, 500_000),
            ..calm
        };
        let stats = run(&settings).stats();
        resumed += stats.parent.resumed;
        outcomes += stats.parent.outcomes;
        found += count(&stats, "found");
        landed += stats.forge.expect("counted").landed;
    }
    assert!(outcomes > 0 && resumed > 0, "creations were asked for again, after their causes: {outcomes}, {resumed}");
    assert!(found > 0, "and found made: {found}");
    assert!(landed > 0, "calls landed after they timed out: {landed}");
}

#[test]
fn pending_reviews_and_ci_run_again_reach_the_inbox() {
    let (mut pending, mut submitted, mut reviews) = (0, 0, 0);
    for seed in 0..1 {
        let calm = Settings::calm(seed);
        let weights = people::Weights { reviews: 16, pushes: 4, ..calm.people.weights };
        let settings = Settings { people: people::Script { weights, ..calm.people }, reruns: 600, ..calm };
        let stats = run(&settings).stats();
        pending += stats.people.pending;
        submitted += stats.people.submits;
        reviews += count(&stats, "news: review");
        assert!(count(&stats, "news: pull") > 0, "seed {seed}: {stats:?}");
    }
    assert!(pending > 0 && submitted > 0, "reviews were started pending and submitted: {pending}, {submitted}");
    assert!(reviews > 0, "verdicts reached the inbox: {reviews}");
}

#[test]
fn a_record_a_person_deleted_holds_its_item_and_is_posted_again() {
    let (mut deleted, mut held) = (0, 0);
    for seed in 0..1 {
        let calm = Settings::calm(seed);
        let weights = people::Weights { deletes: 6, ..calm.people.weights };
        let stats = run(&Settings { people: people::Script { weights, ..calm.people }, ..calm }).stats();
        deleted += stats.people.deletes;
        held += stats.parent.holds;
    }
    assert!(deleted > 0 && held > 0, "records were deleted, and their items held: {deleted}, {held}");
}

#[test]
fn an_item_no_label_finds_is_found_again_by_the_slow_pass_after_a_restart() {
    let mut refound = 0;
    for seed in 0..1 {
        let calm = Settings::calm(seed);
        let weights = people::Weights { removals: 8, ..calm.people.weights };
        let settings = Settings {
            people: people::Script { weights, ..calm.people },
            restarts: 2,
            restart_at: temper_engine_model_forge_tests::Span::millis(200_000, 600_000),
            ..calm
        };
        refound += count(&run(&settings).stats(), "announced: unlabelled");
    }
    assert!(refound > 0, "items carrying neither label were found again");
}

#[test]
fn a_seed_replays_to_the_same_run() {
    let run = |seed: u64| {
        let world = run(&Settings::random(seed));
        (world.trace().to_vec(), (world.stats(), world.now()))
    };
    let trace = assert_replays(7, 8, run);
    assert!(trace.len() > 100, "the world did something");
}

#[test]
fn facts_change_nothing() {
    for seed in 0..5 {
        let settings = Settings::random(seed);
        let none = run(&Settings { limits: Limits { facts: 0, ..settings.limits }, ..settings });
        let many = run(&Settings { limits: Limits { facts: 4096, ..settings.limits }, ..settings });
        assert!(none.trace() == many.trace(), "seed {seed}: the same run whatever facts are kept");
    }
}
