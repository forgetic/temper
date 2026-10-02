//! The whole worker in its world: scenarios, replay, and a sweep of random
//! worlds.

use std::collections::BTreeMap;

use temper_fake_engine_model::{Config, Endings, Tally};
use temper_lib::Duration;
use temper_worker_model::Limits;
use temper_worker_model_agent_tests::script::{self, Fates};
use temper_worker_model_tests::{Git, Network, Settings, Span, Stats, World, translate};

const ITERATIONS: u32 = 2_000_000;

fn run(settings: &Settings) -> World {
    let mut world = World::new(settings.clone());
    world.run(ITERATIONS);
    world
}

fn count(stats: &Stats, kind: &str) -> u32 {
    stats.answers.get(kind).copied().unwrap_or(0)
}

/// The worlds of seeds `0..seeds` under `settings`, each run to its end:
/// their stats and the engine's tallies.
fn worlds(seeds: u64, settings: impl Fn(Settings) -> Settings) -> Vec<(Stats, Tally)> {
    (0..seeds)
        .map(|seed| {
            let world = run(&settings(Settings::calm(seed)));
            (world.stats(), world.tally())
        })
        .collect()
}

/// The sum of `field` over `worlds`.
fn total(worlds: &[(Stats, Tally)], field: impl Fn(&Stats, &Tally) -> u32) -> u32 {
    worlds.iter().map(|(stats, tally)| field(stats, tally)).sum()
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

/// A channel that drops once, `life` after it opens, for `outage`; and runs
/// that take `steps` steps, so that some are live when it drops.
fn dropping(settings: Settings, life: Span, outage: Span, steps: u32) -> Settings {
    Settings {
        network: Network { drops: 1, drop: 1000, life, outage, ..settings.network },
        script: script::Script { steps, ..settings.script },
        ..settings
    }
}

#[test]
fn a_calm_world_answers_every_run_and_settles() {
    let world = run(&Settings::calm(1));
    let stats = world.stats();
    let tally = world.tally();
    assert_eq!(tally.assigned, stats.answers_sent, "every assignment is answered: {stats:?}");
    assert_eq!(stats.answers_lost + stats.held + stats.drops, 0, "the channel never drops: {stats:?}");
    assert!(count(&stats, "ended") > 0 && count(&stats, "parked") > 0, "{stats:?}");
    assert!(stats.landed > 0 && stats.edits > 0, "runs push what they edited: {stats:?}");
    assert!(stats.relayed > 0 && stats.events > 0, "the engine answers relays and sends events: {stats:?}");
}

#[test]
fn a_coding_run_lands_what_its_agent_edited() {
    let worlds = worlds(8, |calm| {
        let calm = fated(calm, Fates { ended: 1, ..NONE });
        Settings {
            engine: Config { writable: 1000, changes: 1000, verdicts: 0, ..calm.engine },
            script: script::Script { calls: 600, pushes: 1000, waits: 0, ..calm.script },
            edits: 1000,
            ..calm
        }
    });
    let landed = total(&worlds, |stats, _| stats.landed);
    let told = total(&worlds, |_, tally| tally.landed);
    assert!(told > 0 && landed >= told, "pushes land, and the engine hears of it: {landed} {told}");
    assert!(total(&worlds, |stats, _| count(stats, "ended")) > 0);
}

#[test]
fn a_parked_run_is_resumed_from_its_snapshot() {
    let worlds = worlds(8, |calm| {
        let calm = fated(calm, Fates { parked: 1, ..NONE });
        Settings {
            engine: Config { resumes: 1000, saves: 0, ..calm.engine },
            script: script::Script { waits: 0, ..calm.script },
            ..calm
        }
    });
    assert!(total(&worlds, |stats, _| count(stats, "parked")) > 0);
    // Each agent started from a snapshot heard exactly its attempt's, which
    // the world checks as it goes.
    assert!(total(&worlds, |_, tally| tally.resumed) > 0, "the engine resumes parked runs");
    assert!(total(&worlds, |stats, _| stats.resumed) > 0, "agents start from their snapshots");
}

#[test]
fn a_parked_run_starts_fresh_from_its_saved_work() {
    let worlds = worlds(8, |calm| {
        let calm = fated(calm, Fates { parked: 1, ..NONE });
        Settings {
            engine: Config { resumes: 0, saves: 1000, writable: 1000, ..calm.engine },
            script: script::Script { calls: 0, waits: 0, ..calm.script },
            scribbles: 1000,
            ..calm
        }
    });
    assert!(total(&worlds, |stats, _| stats.saved) > 0, "parked runs save their work");
    assert!(total(&worlds, |stats, _| stats.from_saved) > 0, "the next attempt starts from it");
    assert_eq!(total(&worlds, |stats, _| stats.resumed), 0);
}

#[test]
fn a_channel_lost_for_less_than_the_grace_keeps_the_runs() {
    let worlds = worlds(8, |calm| dropping(calm, Span::millis(5_000, 30_000), Span::millis(1_000, 20_000), 20));
    assert!(total(&worlds, |stats, _| stats.drops) > 0);
    for (stats, _) in &worlds {
        assert_eq!(count(stats, "cancelled contact"), 0, "{stats:?}");
        assert!(stats.hellos > stats.drops, "the worker says hello again: {stats:?}");
    }
    assert!(total(&worlds, |stats, _| stats.longest_outage.map_or(0, |_| 1)) > 0, "runs went on meanwhile");
}

#[test]
fn a_channel_lost_for_longer_than_the_grace_cancels_the_runs() {
    let worlds = worlds(8, |calm| dropping(calm, Span::millis(5_000, 30_000), Span::millis(90_000, 120_000), 30));
    assert!(total(&worlds, |stats, _| count(stats, "cancelled contact")) > 0);
    let grace = Settings::calm(0).worker.grace;
    let longest = worlds.iter().filter_map(|(stats, _)| stats.longest_outage).max();
    assert!(longest.is_some_and(|longest| longest > grace), "{longest:?}");
}

#[test]
fn an_answer_made_while_the_channel_is_down_follows_the_next_hello() {
    let worlds = worlds(12, |calm| dropping(calm, Span::millis(1_000, 20_000), Span::millis(10_000, 40_000), 4));
    assert!(total(&worlds, |stats, _| stats.held) > 0, "some answer was held");
    for (stats, _) in &worlds {
        assert_eq!(stats.answers_sent, stats.answers_taken + stats.answers_lost, "{stats:?}");
    }
}

#[test]
fn a_shutdown_cancels_the_live_runs_and_ends_once_they_answered() {
    let worlds = worlds(8, |calm| Settings {
        shutdowns: 1000,
        shutdown_at: Span::millis(20_000, 40_000),
        script: script::Script { steps: 20, ..calm.script },
        ..calm
    });
    for (stats, _) in &worlds {
        assert!(stats.shutdown && stats.done, "{stats:?}");
        assert_eq!(stats.abandoned, 0, "the channel was up: no answer was given up");
    }
    assert!(total(&worlds, |stats, _| count(stats, "cancelled shutdown")) > 0);
}

#[test]
fn a_worker_shutting_down_out_of_reach_delivers_its_answers_if_the_channel_opens_before_it_is_done() {
    // The worker is told to shut down with runs live, and the channel drops
    // and stays down past the worker's grace. Some agents wind down at once,
    // and their runs answer; the others ignore the cancel and the terminate,
    // and are killed only once the channel has opened again.
    let worlds = worlds(16, |calm| {
        let calm = dropping(calm, Span::millis(25_000, 35_000), Span::millis(62_000, 75_000), 30);
        let limits = calm.worker;
        Settings {
            worker: Limits {
                agent: temper_worker_model::agent::Limits {
                    grace: Duration::from_secs(80),
                    kill_after: Duration::from_secs(30),
                    ..limits.agent
                },
                ..limits
            },
            engine: Config { window: Duration::from_secs(20), ..calm.engine },
            script: script::Script { deaf_to_cancel: 500, stubborn: 1000, ..calm.script },
            shutdowns: 1000,
            shutdown_at: Span::millis(20_000, 30_000),
            ..calm
        }
    });
    for (stats, _) in &worlds {
        assert!(stats.done, "{stats:?}");
    }
    let delivered = worlds.iter().filter(|(stats, _)| stats.kept_past_grace > 0 && stats.abandoned == 0).count();
    assert!(delivered > 0, "answers kept past the grace, with a run left, are delivered once the channel opens");
}

#[test]
fn a_hung_agent_is_stopped_by_the_watchdog() {
    let worlds = worlds(4, |calm| fated(calm, Fates { hang: 1, ..NONE }));
    assert!(total(&worlds, |stats, _| count(stats, "agent no progress")) > 0);
}

#[test]
fn a_push_finds_its_branch_moved_by_another_party() {
    let worlds = worlds(8, |calm| Settings {
        engine: Config { writable: 1000, changes: 1000, verdicts: 0, ..calm.engine },
        git: Git { advance: 1000, advance_after: Span::millis(0, 1_000), ..calm.git },
        script: script::Script { calls: 600, pushes: 1000, step: Span::millis(2_000, 5_000), ..calm.script },
        edits: 1000,
        ..calm
    });
    assert!(total(&worlds, |stats, _| stats.rejected) > 0, "a push is rejected");
    assert!(total(&worlds, |stats, _| stats.pushed.get("moved").copied().unwrap_or(0)) > 0, "its run is told");
}

#[test]
fn relayed_calls_are_answered_or_withdrawn() {
    let worlds = worlds(8, |calm| Settings {
        engine: Config {
            relay_min: Duration::from_millis(100),
            relay_max: Duration::from_secs(10),
            relay_errors: 300,
            ..calm.engine
        },
        script: script::Script {
            calls: 800,
            pushes: 0,
            blocking: 1000,
            call_deadline: Span::millis(1_000, 5_000),
            ..calm.script
        },
        ..calm
    });
    assert!(total(&worlds, |stats, _| stats.relayed) > 0, "the engine answers");
    assert!(total(&worlds, |stats, _| stats.withdraws) > 0, "runs withdraw what takes too long");
    assert!(total(&worlds, |stats, _| stats.withdrawn) > 0, "and hear so");
    assert!(total(&worlds, |_, tally| tally.errors) > 0);
}

#[test]
fn a_cancelled_runs_relayed_calls_are_answered_unavailable() {
    let worlds = worlds(8, |calm| Settings {
        engine: Config {
            cancels: 1000,
            cancel_min: Duration::from_secs(1),
            cancel_max: Duration::from_secs(10),
            relay_min: Duration::from_secs(5),
            relay_max: Duration::from_secs(20),
            ..calm.engine
        },
        script: script::Script {
            calls: 800,
            pushes: 0,
            blocking: 1000,
            call_deadline: Span::millis(30_000, 60_000),
            ..calm.script
        },
        ..calm
    });
    assert!(total(&worlds, |stats, _| count(stats, "cancelled engine")) > 0);
    assert!(total(&worlds, |stats, _| stats.unavailable) > 0);
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
    let dropped = run(&Settings { worker: none, ..settings });
    assert_eq!(kept.trace(), dropped.trace(), "nothing depends on whether a fact is kept");
    assert_eq!(kept.tally(), dropped.tally());
    let (kept, dropped) = (kept.stats(), dropped.stats());
    assert!(dropped.facts_lost > 0 && kept.facts_lost == 0, "{dropped:?}");
    assert_eq!(Stats { facts: 0, facts_lost: 0, ..kept }, Stats { facts: 0, facts_lost: 0, ..dropped });
}

#[test]
fn a_seed_replays_to_the_same_run() {
    let trace = temper_world::assert_replays(11, 12, |seed| {
        let world = run(&Settings::calm(seed));
        (world.trace().to_vec(), (world.stats(), world.tally(), world.now()))
    });
    assert!(trace.len() > 100, "the world did something");
}

#[test]
fn random_worlds_settle_and_reach_every_ending() {
    let mut answers = BTreeMap::new();
    let mut endings = Endings { finished: 0, rejected: 0, held: 0, parked: 0, cancelled: 0 };
    for seed in 0..400 {
        let world = run(&Settings::rough(seed));
        for (kind, count) in world.stats().answers {
            *answers.entry(kind).or_insert(0) += count;
        }
        let ended = world.tally().endings;
        endings.finished += ended.finished;
        endings.rejected += ended.rejected;
        endings.held += ended.held;
        endings.parked += ended.parked;
        endings.cancelled += ended.cancelled;
    }
    // A run says it was cancelled only once something cancelled it, and the
    // worker reports that cancel as its own: the engine's, lost contact, a
    // shutdown, or the wall time of its agent.
    assert!(!answers.contains_key("run cancelled"), "no run's cancel is its own: {answers:?}");
    for kind in translate::ANSWER_KINDS.into_iter().filter(|kind| *kind != "run cancelled") {
        assert!(answers.contains_key(kind), "some run is answered {kind}: {answers:?}");
    }
    let Endings { finished, rejected, held, parked, cancelled } = endings;
    assert!(finished > 0 && rejected > 0 && held > 0 && parked > 0 && cancelled > 0, "every ending: {endings:?}");
}
