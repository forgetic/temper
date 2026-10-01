//! End to end at the model layer: the agent's model and a fake provider's
//! model, talking through a simulated world.

use temper_agent_model::llm::{Block, Failure};
use temper_agent_model::{Limits, Outcome, Report};
use temper_agent_model_tests::{Settings, Span, World, task};
use temper_lib::{Duration, Rng, Time};
use temper_llm_model::Config;

const ITERATIONS: u32 = 100_000;

fn done() -> Outcome {
    Outcome::Done { content: Box::new([Block::Text { text: b"done"[..].into() }]) }
}

fn outcome(world: &World, run: u64) -> &Outcome {
    match world.report(run).expect("every run is answered") {
        Report::Ended { outcome, .. } => outcome,
        report @ (Report::Busy | Report::Invalid) => panic!("run {run} was refused: {report:?}"),
    }
}

fn turns(world: &World, run: u64) -> u32 {
    match world.report(run).expect("every run is answered") {
        Report::Ended { turns, .. } => *turns,
        report @ (Report::Busy | Report::Invalid) => panic!("run {run} was refused: {report:?}"),
    }
}

#[test]
fn a_session_runs_its_tool_rounds_and_ends_with_the_providers_answer() {
    let mut world = World::new(Settings::calm(1));
    let run = world.submit(Time::ZERO, task(b"fix the build"));
    world.run(ITERATIONS);

    // Two rounds of tool calls, then the answer.
    assert_eq!(outcome(&world, run), &done());
    assert_eq!(turns(&world, run), 3);
    let stats = world.stats();
    assert_eq!((stats.calls, stats.provider_calls, stats.tool_runs), (3, 3, 2));
}

#[test]
fn sessions_run_side_by_side_and_runs_beyond_the_slots_are_refused() {
    let calm = Settings::calm(2);
    let mut world = World::new(Settings { agent: Limits { sessions: 2, ..calm.agent }, ..calm });
    let runs: Vec<u64> = (0..5).map(|_| world.submit(Time::ZERO, task(b"refactor"))).collect();
    world.run(ITERATIONS);

    let mut busy = 0;
    for run in runs {
        match world.report(run).expect("every run is answered") {
            Report::Busy => busy += 1,
            Report::Ended { outcome, .. } => assert_eq!(outcome, &done()),
            Report::Invalid => panic!("the task fits the limits"),
        }
    }
    assert_eq!(busy, 3);
}

#[test]
fn transient_failures_are_retried_until_calls_succeed() {
    let calm = Settings::calm(3);
    let settings = Settings {
        agent: Limits { retries: 10, ..calm.agent },
        provider: Config { overloaded: 200, rate_limited: 100, ..calm.provider },
        ..calm
    };
    let mut world = World::new(settings);
    let runs: Vec<u64> = (0..4).map(|_| world.submit(Time::ZERO, task(b"add a test"))).collect();
    world.run(ITERATIONS);

    for &run in &runs {
        assert_eq!(outcome(&world, run), &done());
    }
    let completions: u32 = runs.iter().map(|&run| turns(&world, run)).sum();
    assert!(world.stats().provider_calls > completions, "some calls were retried");
}

#[test]
fn a_session_fails_once_its_retries_run_out() {
    let calm = Settings::calm(4);
    let settings = Settings { provider: Config { overloaded: 1000, ..calm.provider }, ..calm };
    let mut world = World::new(settings);
    let run = world.submit(Time::ZERO, task(b"fix the build"));
    world.run(ITERATIONS);

    assert_eq!(outcome(&world, run), &Outcome::Failed { failure: Failure::Overloaded });
    assert_eq!(world.stats().calls, calm.agent.retries + 1);
}

#[test]
fn calls_slower_than_their_timeout_time_out_and_their_late_answers_are_dropped() {
    let calm = Settings::calm(5);
    let settings = Settings {
        agent: Limits { call_timeout: Duration::from_secs(1), retries: 2, ..calm.agent },
        provider: Config {
            latency_min: Duration::from_secs(10),
            latency_max: Duration::from_secs(10),
            ..calm.provider
        },
        ..calm
    };
    let mut world = World::new(settings);
    let run = world.submit(Time::ZERO, task(b"fix the build"));
    world.run(ITERATIONS);

    assert_eq!(outcome(&world, run), &Outcome::Failed { failure: Failure::TimedOut });
    let stats = world.stats();
    assert_eq!((stats.calls, stats.timeouts, stats.late_answers), (3, 3, 3));
}

#[test]
fn an_expiring_session_cancels_its_call_in_flight() {
    let calm = Settings::calm(6);
    let settings = Settings {
        agent: Limits { session_timeout: Duration::from_secs(10), ..calm.agent },
        provider: Config {
            latency_min: Duration::from_secs(30),
            latency_max: Duration::from_secs(30),
            ..calm.provider
        },
        ..calm
    };
    let mut world = World::new(settings);
    let run = world.submit(Time::ZERO, task(b"fix the build"));
    world.run(ITERATIONS);

    assert_eq!(outcome(&world, run), &Outcome::Expired);
    let stats = world.stats();
    assert_eq!((stats.cancels, stats.late_answers), (1, 1));
}

#[test]
fn an_expiring_session_cancels_its_tool_in_flight() {
    let calm = Settings::calm(7);
    let settings = Settings {
        agent: Limits { session_timeout: Duration::from_secs(10), ..calm.agent },
        tool: Span::millis(60_000, 60_000),
        ..calm
    };
    let mut world = World::new(settings);
    let run = world.submit(Time::ZERO, task(b"fix the build"));
    world.run(ITERATIONS);

    assert_eq!(outcome(&world, run), &Outcome::Expired);
    assert_eq!(turns(&world, run), 1);
    assert_eq!(world.stats().tool_cancels, 1);
}

#[test]
fn the_turn_limit_ends_a_session_that_keeps_calling_tools() {
    let calm = Settings::calm(8);
    let settings = Settings {
        agent: Limits { turns: 3, ..calm.agent },
        provider: Config { tool_rounds: 100, ..calm.provider },
        ..calm
    };
    let mut world = World::new(settings);
    let run = world.submit(Time::ZERO, task(b"fix the build"));
    world.run(ITERATIONS);

    assert_eq!(outcome(&world, run), &Outcome::TurnLimit);
    assert_eq!(turns(&world, run), 3);
}

#[test]
fn a_seed_replays_to_the_same_run() {
    let replay = |seed| {
        let mut world = World::new(noisy(seed));
        submit_noisily(&mut world, seed);
        world.run(ITERATIONS);
        (world.trace().to_vec(), world.stats(), world.now())
    };
    let (trace, stats, end) = replay(9);
    assert!(trace.len() > 20, "the run did something");
    assert_eq!(replay(9), (trace.clone(), stats, end));
    assert_ne!(replay(10).0, trace);
}

/// Hundreds of worlds with random limits, faults and schedules: each settles,
/// with every run answered once and nothing left alive or in flight (checked by
/// `World::run`), and between them they reach every way a session can end.
#[test]
fn random_worlds_settle_with_every_run_answered() {
    let mut seen = std::collections::BTreeSet::new();
    for seed in 0..300 {
        let mut world = World::new(noisy(seed));
        submit_noisily(&mut world, seed);
        world.run(ITERATIONS);
        for (_, report) in world.reports() {
            let kind = match report.expect("every run is answered") {
                Report::Busy => "busy",
                Report::Invalid => "invalid",
                Report::Ended { outcome, .. } => match outcome {
                    Outcome::Done { .. } => "done",
                    Outcome::Failed { failure: Failure::TimedOut } => "timed out",
                    Outcome::Failed { .. } => "failed",
                    Outcome::Expired => "expired",
                    Outcome::TurnLimit => "turn limit",
                    Outcome::TranscriptFull => "transcript full",
                    Outcome::Truncated | Outcome::Refused | Outcome::Malformed => "provider",
                },
            };
            seen.insert(kind);
        }
    }
    let expected = ["busy", "done", "expired", "failed", "timed out", "transcript full", "turn limit"];
    assert_eq!(seen.into_iter().collect::<Vec<_>>(), {
        let mut expected = expected.to_vec();
        expected.sort_unstable();
        expected
    });
}

/// Settings drawn from `seed`: small limits, faults, and latencies that race
/// the deadlines.
fn noisy(seed: u64) -> Settings {
    let mut rng = Rng::new(seed);
    let calm = Settings::calm(seed);
    let mut millis = |low: u64, high: u64| Duration::from_millis(rng.between(low, high));
    let call_timeout = millis(500, 5_000);
    let session_timeout = millis(2_000, 120_000);
    let latency_max = millis(10, 4_000);
    let mut rng = Rng::new(seed.wrapping_add(1));
    let mut pick = |low: u64, high: u64| u32::try_from(rng.between(low, high)).expect("small numbers");
    Settings {
        agent: Limits {
            sessions: pick(1, 4),
            messages: pick(4, 16),
            session_bytes: u64::from(pick(2_000, 8_000)),
            turns: pick(1, 6),
            retries: pick(0, 4),
            backoff_base: Duration::from_millis(50),
            backoff_max: Duration::from_secs(2),
            call_timeout,
            session_timeout,
            ..calm.agent
        },
        provider: Config {
            calls: pick(1, 8),
            latency_min: Duration::from_millis(10),
            latency_max,
            overloaded: pick(0, 300),
            rate_limited: pick(0, 200),
            tool_rounds: pick(0, 5),
            ..calm.provider
        },
        tool: Span::millis(10, 3_000),
        tool_errors: pick(0, 300),
        ..calm
    }
}

/// Up to eight runs at random times in the first minute.
fn submit_noisily(world: &mut World, seed: u64) {
    let mut rng = Rng::new(seed.wrapping_add(2));
    for _ in 0..rng.between(1, 8) {
        let at = Time::from_nanos(rng.between(0, 60_000_000_000));
        world.submit(at, task(b"make the tests pass"));
    }
}
