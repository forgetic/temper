//! The whole worker in its world, against the engine: scenarios, replay,
//! and a sweep of random worlds.

use temper_engine_model_tests::people::Story;
use temper_lib::Duration;
use temper_worker_model::Limits;
use temper_worker_model_agent_tests::script::{self, Fates};
use temper_worker_model_tests::{Git, Network, Settings, Span, Stats, World};

const ITERATIONS: u32 = 300_000;

fn run(settings: &Settings) -> World {
    let mut world = World::new(settings.clone());
    world.run(ITERATIONS);
    world
}

fn count(stats: &Stats, kind: &str) -> u32 {
    stats.answers.get(kind).copied().unwrap_or(0)
}

fn ending(stats: &Stats, name: &str) -> u32 {
    stats.endings.get(name).copied().unwrap_or(0)
}

/// The worlds of seeds `0..seeds` under `settings`, each run to its end:
/// their stats.
fn worlds(seeds: u64, settings: impl Fn(Settings) -> Settings) -> Vec<Stats> {
    (0..seeds).map(|seed| run(&settings(Settings::calm(seed))).stats()).collect()
}

/// The sum of `field` over `worlds`.
fn total(worlds: &[Stats], field: impl Fn(&Stats) -> u32) -> u32 {
    worlds.iter().map(field).sum()
}

/// Whether the item of the story `tale` is closed.
fn closed(world: &World, tale: usize) -> bool {
    let item = world.item(tale).expect("the story's item is known");
    let name = temper_engine_model_tests::deployment::name(item.repository);
    !world.mirror().issue(name, item.number).expect("on the forge").open
}

const NONE: Fates = Fates {
    ended: 0,
    parked: 0,
    failed: 0,
    crash: 0,
    hang: 0,
    overrun: 0,
    garbage: 0,
    duplicate: 0,
    trailing: 0,
    oversized: 0,
    deaf: 0,
    mute: 0,
};

/// Agents that meet the fates given.
fn fated(settings: Settings, fates: Fates) -> Settings {
    Settings { script: script::Script { fates, ..settings.script }, ..settings }
}

/// A channel that drops up to three times, each `life` after it opens, for
/// `outage`; and runs that take `steps` steps of their own, so that some
/// are live when it drops.
fn dropping(settings: Settings, life: Span, outage: Span, steps: u32) -> Settings {
    Settings {
        network: Network { drops: 3, drop: 1000, life, outage, ..settings.network },
        script: script::Script { steps, ..settings.script },
        ..settings
    }
}

#[test]
fn a_calm_world_answers_every_run_and_settles() {
    let world = run(&Settings::calm(1));
    for tale in 0..world.stories() {
        assert!(closed(&world, tale), "story {tale} is done");
    }
    let stats = world.stats();
    assert_eq!(stats.answers_sent, stats.answers_taken, "every answer reaches the engine: {stats:?}");
    assert_eq!(stats.answers_lost + stats.held + stats.drops, 0, "the channel never drops: {stats:?}");
    assert!(count(&stats, "ended") > 0 && count(&stats, "parked") > 0, "{stats:?}");
    assert!(stats.landed > 0 && stats.edits > 0, "runs push what they edited: {stats:?}");
    assert!(stats.relayed > 0 && stats.events > 0, "the engine answers relays and sends events: {stats:?}");
    assert!(ending(&stats, "merged") > 0, "a change landed: {stats:?}");
}

#[test]
fn an_issue_handed_in_is_fixed_by_what_its_agents_pushed() {
    let worlds = worlds(4, |calm| Settings { stories: vec![Story::Fix], ..calm });
    assert!(total(&worlds, |stats| stats.landed) >= 8, "each change is pushed, then repaired");
    assert_eq!(total(&worlds, |stats| ending(stats, "merged")), 4, "and lands once reviewed");
    assert!(total(&worlds, |stats| ending(stats, "reviewed")) >= 4);
}

#[test]
fn a_chatting_session_parks_and_resumes_from_its_snapshot() {
    let worlds = worlds(4, |calm| Settings { stories: vec![Story::Chat], ..calm });
    assert!(total(&worlds, |stats| count(stats, "parked")) >= 4, "each session parks");
    assert!(total(&worlds, |stats| stats.resumed) >= 4, "and its next run starts from its snapshot");
    assert!(total(&worlds, |stats| stats.events) >= 4, "a person's message reaches a waiting run");
}

#[test]
fn a_channel_lost_for_less_than_the_grace_keeps_the_runs() {
    let worlds = worlds(1, |calm| dropping(calm, Span::millis(20_000, 120_000), Span::millis(1_000, 20_000), 20));
    assert!(total(&worlds, |stats| stats.drops) > 0);
    for stats in &worlds {
        assert_eq!(count(stats, "cancelled contact"), 0, "{stats:?}");
        assert!(stats.hellos > stats.drops, "the worker says hello again: {stats:?}");
    }
    assert!(total(&worlds, |stats| stats.longest_outage.map_or(0, |_| 1)) > 0, "runs went on meanwhile");
}

/// The channel lost past the grace: the worker cancels its runs. Past the
/// engine's grace too, a change's attempt may have pushed before it was
/// presumed lost: the engine learns it from the answer that comes late, and
/// the world settles.
#[test]
fn a_channel_lost_for_longer_than_the_grace_cancels_the_runs() {
    let mut cancelled = 0;
    let mut longest = None;
    for seed in 0..1 {
        let calm = Settings::calm(seed);
        let settings = dropping(calm, Span::millis(20_000, 120_000), Span::millis(90_000, 120_000), 30);
        let mut world = World::new(settings);
        world.run(ITERATIONS);
        let stats = world.stats();
        cancelled += count(&stats, "cancelled contact");
        longest = longest.max(stats.longest_outage);
    }
    assert!(cancelled > 0);
    let grace = Settings::calm(0).worker.grace;
    assert!(longest.is_some_and(|longest| longest > grace), "{longest:?}");
}

#[test]
fn an_answer_made_while_the_channel_is_down_follows_the_next_hello() {
    let worlds = worlds(1, |calm| dropping(calm, Span::millis(1_000, 20_000), Span::millis(10_000, 40_000), 4));
    assert!(total(&worlds, |stats| stats.held) > 0, "some answer was held");
    for stats in &worlds {
        assert_eq!(stats.answers_sent, stats.answers_taken + stats.answers_lost, "{stats:?}");
    }
}

/// Frames sent again change nothing: a world whose protocol layers send
/// frames again, at every moment a frame allows, ends as the same world does
/// without them, but for the engine acknowledging answers again.
#[test]
fn frames_sent_again_change_no_outcome() {
    for seed in 0..2 {
        let calm = Settings { stops: 1000, stop_after: Span::millis(1_000, 5_000), ..Settings::calm(seed) };
        let again = run(&Settings { network: Network { duplicates: 1000, ..calm.network }, ..calm.clone() });
        let once = run(&calm);
        let (with, without) = (again.stats(), once.stats());
        assert!(with.answers_copied > 0 && without.duplicated == 0, "{with:?}");
        assert!(with.late.get("relayed").is_some_and(|late| *late > 0), "some copies come late: {:?}", with.late);
        assert_eq!(with.answers, without.answers, "seed {seed}: the worker answers the same");
        let endings = |stats: &Stats| {
            let mut endings = stats.endings.clone();
            endings.remove("acknowledged");
            endings
        };
        assert_eq!(endings(&with), endings(&without), "seed {seed}: the engine does the same");
        let mirror = |world: &World| format!("{:?}", world.mirror());
        assert_eq!(mirror(&again), mirror(&once), "seed {seed}: the forge ends the same");
        assert_eq!(again.now(), once.now(), "seed {seed}: at the same moment");
    }
}

/// Copies that come late, on a channel that drops and stalls, of runs that
/// a person stops and that wind down slowly: an engine's cancel, an answer
/// to a relayed call and an inbound event once the attempt they are for has
/// answered, and a cancel again behind the hello that lists its attempt. The
/// worker takes each again harmlessly, and the world settles.
#[test]
fn copies_that_come_late_are_taken_again_harmlessly() {
    let calm = Settings::calm(1);
    let limits = calm.worker;
    let world = run(&Settings {
        worker: Limits {
            agent: temper_worker_model::agent::Limits { grace: Duration::from_secs(40), ..limits.agent },
            ..limits
        },
        stops: 500,
        stop_after: Span::millis(1_000, 5_000),
        network: Network {
            duplicates: 1000,
            stalls: 100,
            drops: 10,
            drop: 1000,
            life: Span::millis(3_000, 15_000),
            outage: Span::millis(1_000, 10_000),
            ..calm.network
        },
        script: script::Script { steps: 20, deaf_to_cancel: 1000, ..calm.script },
        ..calm
    });
    let stats = world.stats();
    for kind in ["cancel", "inbound", "relayed"] {
        assert!(stats.late.get(kind).is_some_and(|late| *late > 0), "a late {kind}: {:?}", stats.late);
    }
    assert!(stats.recancels > 0, "a cancel again behind a hello");
    assert!(stats.answers_copied > 0 && stats.stalled > 0, "{stats:?}");
}

#[test]
fn a_shutdown_cancels_the_live_runs_and_a_new_worker_takes_over() {
    let worlds = worlds(1, |calm| Settings {
        shutdowns: 1000,
        shutdown_at: Span::millis(20_000, 40_000),
        script: script::Script { steps: 20, ..calm.script },
        ..calm
    });
    for stats in &worlds {
        assert!(stats.shutdown && stats.done && stats.comebacks == 1, "{stats:?}");
        assert_eq!(stats.abandoned, 0, "the channel was up: no answer was given up");
    }
    assert!(total(&worlds, |stats| count(stats, "cancelled shutdown")) > 0);
}

#[test]
fn a_worker_shutting_down_out_of_reach_delivers_its_answers_if_the_channel_opens_before_it_is_done() {
    // The worker is told to shut down with runs live, and the channel drops
    // and stays down past the worker's grace. Some agents wind down at once,
    // and their runs answer; the others ignore the cancel and the terminate,
    // and are killed only once the channel has opened again; out of reach
    // past the engine's grace too, the world settles. The first seed whose
    // world keeps an answer past the grace with a run left.
    let calm = dropping(Settings::calm(3), Span::millis(60_000, 70_000), Span::millis(62_000, 75_000), 30);
    let limits = calm.worker;
    let mut world = World::new(Settings {
        worker: Limits {
            agent: temper_worker_model::agent::Limits {
                grace: Duration::from_secs(80),
                kill_after: Duration::from_secs(30),
                ..limits.agent
            },
            ..limits
        },
        script: script::Script { deaf_to_cancel: 500, stubborn: 1000, ..calm.script },
        shutdowns: 1000,
        shutdown_at: Span::millis(55_000, 65_000),
        ..calm
    });
    world.run(ITERATIONS);
    let stats = world.stats();
    assert!(stats.done, "{stats:?}");
    assert!(
        stats.kept_past_grace > 0 && stats.abandoned == 0,
        "answers kept past the grace, with a run left, are delivered once the channel opens: {stats:?}"
    );
}

/// The engine restarts, cold, twice while runs are live: the worker dials
/// the new engine, says what it hosts and sends again what the last did not
/// acknowledge, and the world settles.
#[test]
fn an_engine_that_restarts_takes_the_runs_back_from_the_worker() {
    let worlds = worlds(2, |calm| Settings {
        restarts: 2,
        restart_at: Span::millis(30_000, 300_000),
        script: script::Script { steps: 20, ..calm.script },
        ..calm
    });
    assert_eq!(total(&worlds, |stats| stats.restarts), 4);
    assert!(total(&worlds, |stats| stats.hellos) > 4, "the worker says hello to each engine");
    assert!(total(&worlds, |stats| ending(stats, "story closed")) > 0);
}

/// Agents that say what no engine reads: some end with the outcome their
/// script wrote, and some, fated to write garbage, relay the calls their
/// script wrote. The protocol layer answers such a call itself, and takes
/// such an outcome as the agent's failure; their runs are tried again, and
/// the world settles.
#[test]
fn words_no_engine_reads_fail_their_runs_which_are_tried_again() {
    let calm = Settings::calm(0);
    let fates = Fates { garbage: 2, ..calm.script.fates };
    let world = run(&Settings { garbled: 300, script: script::Script { fates, ..calm.script }, ..calm });
    let stats = world.stats();
    assert!(ending(&stats, "undecoded") > 0 && ending(&stats, "undecodable") > 0, "{:?}", stats.endings);
}

/// The engine restarts as soon as it posts a run's outcome, three times,
/// while it applies it: the engine that starts cold reads the outcome posted
/// and applies it, and the change lands.
#[test]
fn an_engine_that_restarts_while_it_applies_an_outcome_resumes_it() {
    let worlds =
        worlds(2, |calm| Settings { stories: vec![Story::Fix], applying_restarts: 3, restart_applying: 1000, ..calm });
    assert_eq!(total(&worlds, |stats| stats.restarts), 6);
    assert_eq!(total(&worlds, |stats| ending(stats, "merged")), 2, "and each change lands");
}

#[test]
fn a_hung_agent_is_stopped_by_the_watchdog() {
    let worlds = worlds(1, |calm| fated(calm, Fates { ended: 2, hang: 1, ..NONE }));
    assert!(total(&worlds, |stats| count(stats, "agent no progress")) > 0);
}

/// Another party moves the branch of every change once, while its run
/// works: the run's push is rejected, and its run told; the engine learns
/// where the branch went, and a later run lands the change.
#[test]
fn a_push_finds_its_branch_moved_by_another_party() {
    let worlds = worlds(1, |calm| moving(calm.seed, 1));
    assert!(total(&worlds, |stats| stats.rejected) > 0, "a push is rejected");
    assert!(total(&worlds, |stats| stats.pushed.get("moved").copied().unwrap_or(0)) > 0, "its run is told");
    assert_eq!(total(&worlds, |stats| ending(stats, "merged")), 1, "and the change lands");
}

/// The same, the branch moved under every run: each run's push is rejected
/// and its outcome is stale. A repair whose outcome went stale counts all
/// the same, so the change is held once its repairs are spent.
#[test]
fn a_change_whose_branch_moves_under_every_run_is_run_a_bounded_number_of_times() {
    let mut world = World::new(moving(0, u32::MAX));
    world.run_for(Duration::from_secs(3 * 3_600), ITERATIONS);
    let stats = world.stats();
    assert!(stats.rejected > 0, "a push is rejected");
    let assigned = ending(&stats, "assigned");
    assert!(assigned < 20, "the change runs a bounded number of times, not {assigned}");
}

/// A fix whose branch another party moves `moves` times, each while one of
/// its runs works.
fn moving(seed: u64, moves: u32) -> Settings {
    let calm = Settings::only(seed, &[Story::Fix]);
    Settings {
        git: Git { advance: 1000, advance_after: Span::millis(0, 1_000), moves, ..calm.git },
        script: script::Script { step: Span::millis(2_000, 5_000), ..calm.script },
        ..calm
    }
}

/// A change into a base the forge does not have: its first checkout creates
/// the base from the default branch, and the forge refuses some creations,
/// which the engine tries again; the change lands into the base.
#[test]
fn a_change_into_a_base_the_forge_lacks_creates_it_and_lands_there() {
    let worlds = worlds(3, |calm| Settings {
        stories: vec![Story::Fix],
        release: true,
        git: Git { refusing_creates: 500, ..calm.git },
        ..calm
    });
    assert!(total(&worlds, |stats| count(stats, "unprepared refused")) > 0, "a creation was refused");
    assert_eq!(total(&worlds, |stats| ending(stats, "merged")), 3, "and each change lands");
}

/// Another party deletes a change's branch between its attempts, once:
/// its pull request, if it had one, closes with it. The engine finds the
/// branch gone, makes the change again from its base, as a rebase, and
/// reopens its pull request once it is pushed again.
#[test]
fn a_change_whose_branch_another_party_deleted_starts_over() {
    let worlds = worlds(4, |calm| Settings {
        stories: vec![Story::Fix],
        git: Git { deletes: 1000, advance_after: Span::millis(0, 1_000), moves: 1, ..calm.git },
        ..calm
    });
    assert!(total(&worlds, |stats| stats.deleted) > 0);
    assert_eq!(total(&worlds, |stats| ending(stats, "merged")), 4, "each change lands all the same");
}

#[test]
fn relayed_calls_are_answered_or_withdrawn() {
    let worlds = worlds(1, |calm| Settings {
        script: script::Script {
            calls: 800,
            pushes: 0,
            blocking: 1000,
            call_deadline: Span::millis(100, 1_000),
            ..calm.script
        },
        ..calm
    });
    assert!(total(&worlds, |stats| stats.relayed) > 0, "the engine answers");
    assert!(total(&worlds, |stats| stats.withdraws) > 0, "runs withdraw what takes too long");
    assert!(total(&worlds, |stats| stats.withdrawn) > 0, "and hear so");
}

#[test]
fn a_run_a_person_stops_is_cancelled_and_its_calls_answered_unavailable() {
    let worlds = worlds(2, |calm| Settings {
        stories: vec![Story::Fix],
        stops: 1000,
        stop_after: Span::millis(1_000, 10_000),
        // The forge slow to answer the runs' reads, so that some are in
        // flight when their run is cancelled.
        forge: temper_forge_model::Config {
            latency_min: Duration::from_secs(2),
            latency_max: Duration::from_secs(8),
            ..calm.forge
        },
        script: script::Script { calls: 800, pushes: 200, blocking: 1000, ..calm.script },
        ..calm
    });
    assert!(total(&worlds, |stats| count(stats, "cancelled engine")) > 0);
    assert!(total(&worlds, |stats| ending(stats, "stopped")) > 0);
    assert!(total(&worlds, |stats| ending(stats, "released")) > 0, "and the caretaker releases it");
    assert!(total(&worlds, |stats| stats.unavailable) > 0);
}

/// A worker that takes charters smaller than the engine's sessions give
/// refuses them; it shuts down a while later, and the worker that takes
/// over takes them.
/// Sessions a person stops: each is held, released by the caretaker, and
/// woken by its person's message, and it finishes.
#[test]
fn a_session_a_person_stops_is_released_and_woken_by_a_message() {
    let worlds = worlds(2, |calm| Settings {
        stories: vec![Story::Hello, Story::Chat],
        stops: 1000,
        stop_after: Span::millis(1_000, 5_000),
        ..calm
    });
    assert!(total(&worlds, |stats| ending(stats, "stopped")) > 0);
    assert!(total(&worlds, |stats| ending(stats, "woken")) > 0, "and woken once released");
}

#[test]
fn an_assignment_beyond_the_workers_limits_is_refused_invalid() {
    let settings = Settings::only(3, &[Story::Hello]);
    let limits = settings.worker;
    let small = Limits { host: temper_worker_model::host::Limits { charter_bytes: 256, ..limits.host }, ..limits };
    let mut world = World::new(Settings {
        worker: small,
        upgrade: Some(limits),
        shutdowns: 1000,
        shutdown_at: Span::millis(60_000, 90_000),
        ..settings
    });
    world.run(ITERATIONS);
    let stats = world.stats();
    assert!(count(&stats, "refused invalid") > 0 && count(&stats, "ended") > 0, "{:?}", stats.answers);
}

#[test]
fn facts_change_nothing_when_none_are_kept() {
    let settings = Settings::rough(13);
    let limits = settings.worker;
    let none = Limits {
        host: temper_worker_model::host::Limits { facts: 0, ..limits.host },
        checkout: temper_worker_model::checkout::Limits { facts: 0, ..limits.checkout },
        agent: temper_worker_model::agent::Limits { facts: 0, ..limits.agent },
        ..limits
    };
    let kept = run(&settings);
    let dropped = run(&Settings {
        worker: none,
        engine: temper_engine_model::Limits { facts: 0, ..settings.engine },
        ..settings.clone()
    });
    assert_eq!(kept.trace(), dropped.trace(), "nothing depends on whether a fact is kept");
    let (kept, dropped) = (kept.stats(), dropped.stats());
    assert!(dropped.facts_lost > 0 && kept.facts_lost == 0, "{dropped:?}");
    let blind = |stats: Stats| Stats { facts: 0, facts_lost: 0, engine_facts: 0, engine_facts_lost: 0, ..stats };
    assert_eq!(blind(kept), blind(dropped));
}

#[test]
fn a_seed_replays_to_the_same_run() {
    let trace = temper_world::assert_replays(11, 12, |seed| {
        let world = run(&Settings::rough(seed));
        (world.trace().to_vec(), (world.stats(), world.now()))
    });
    assert!(trace.len() > 100, "the world did something");
}

/// A session whose runs fail until it is held, then released: the release
/// wakes it, though it cleared the turn it had claimed, and it runs again,
/// until a run of it ends.
#[test]
fn a_session_released_after_its_runs_failed_runs_again() {
    let settings = fated(Settings::only(2, &[Story::Hello]), Fates { ended: 1, failed: 2, ..NONE });
    let mut world = World::new(Settings { engine: temper_engine_model_tests::deployment::LIMITS, ..settings });
    world.run(ITERATIONS);
    let stats = world.stats();
    assert!(ending(&stats, "released") > 0, "the caretaker released it: {stats:?}");
    assert!(ending(&stats, "assigned") > 3, "and it ran again: {:?}", stats.endings);
}
