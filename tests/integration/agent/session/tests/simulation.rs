//! End to end at the session sub-model: the agent's sessions, their opener and
//! a fake provider's model, talking through a simulated world.

use std::collections::BTreeSet;

use temper_agent_model_session::llm::Failure;
use temper_agent_model_session::{End, Limits, Yield};
use temper_agent_model_session_tests::{Count, Ended, Settings, Span, World, spec};
use temper_lib::{Duration, Rng, Time};
use temper_llm_model::Config;

const ITERATIONS: u32 = 100_000;

fn ended(world: &World, opener: u64) -> Ended {
    world.session(opener).ended.expect("every session ends")
}

fn end(world: &World, opener: u64) -> End {
    ended(world, opener).end
}

fn turns(world: &World, opener: u64) -> u32 {
    ended(world, opener).turns
}

/// The yields of a session: why, and what the LLM said.
fn yields(world: &World, opener: u64) -> Vec<(Yield, &[u8])> {
    world.session(opener).yields.iter().map(|(stop, text)| (*stop, &**text)).collect()
}

#[test]
fn a_session_runs_its_tool_rounds_and_yields_with_the_providers_answer() {
    let mut world = World::new(Settings::calm(1));
    let opener = world.submit(Time::ZERO, spec(b"fix the build"));
    world.run(ITERATIONS);

    // Two rounds of tool calls, then the answer; the opener closes it then.
    assert_eq!(yields(&world, opener), [(Yield::Done, &b"done"[..])]);
    assert_eq!((end(&world, opener), turns(&world, opener)), (End::Closed, 3));
    let stats = world.stats();
    assert_eq!((stats.calls, stats.provider_calls, stats.tool_runs, stats.closes), (3, 3, 2, 1));
}

#[test]
fn a_nudged_session_goes_on_until_its_opener_closes_it() {
    let calm = Settings::calm(2);
    let mut world = World::new(Settings { nudges: Count { min: 2, max: 2 }, ..calm });
    let opener = world.submit(Time::ZERO, spec(b"fix the build"));
    world.run(ITERATIONS);

    // Each message, the first and both nudges, gets two rounds of tools and an
    // answer.
    assert_eq!(yields(&world, opener), [(Yield::Done, &b"done"[..]); 3]);
    assert_eq!((end(&world, opener), turns(&world, opener)), (End::Closed, 9));
    let stats = world.stats();
    assert_eq!((stats.continues, stats.tool_runs, stats.closes), (2, 6, 1));
}

#[test]
fn sessions_run_side_by_side_and_opens_beyond_the_slots_are_refused() {
    let calm = Settings::calm(3);
    let mut world = World::new(Settings { agent: Limits { sessions: 2, ..calm.agent }, ..calm });
    let openers: Vec<u64> = (0..5).map(|_| world.submit(Time::ZERO, spec(b"refactor"))).collect();
    world.run(ITERATIONS);

    let mut busy = 0;
    for opener in openers {
        if end(&world, opener) == End::Busy {
            busy += 1;
            assert!(world.session(opener).session.is_none(), "a refused session never opens");
        } else {
            assert_eq!(end(&world, opener), End::Closed, "the sessions run to their answer");
            assert_eq!(yields(&world, opener), [(Yield::Done, &b"done"[..])]);
        }
    }
    assert_eq!(busy, 3);
}

#[test]
fn transient_failures_are_retried_until_calls_succeed() {
    let calm = Settings::calm(4);
    let settings = Settings {
        agent: Limits { retries: 10, ..calm.agent },
        provider: Config { overloaded: 200, rate_limited: 100, ..calm.provider },
        ..calm
    };
    let mut world = World::new(settings);
    let openers: Vec<u64> = (0..4).map(|_| world.submit(Time::ZERO, spec(b"add a test"))).collect();
    world.run(ITERATIONS);

    for &opener in &openers {
        assert_eq!(yields(&world, opener), [(Yield::Done, &b"done"[..])]);
        assert_eq!(end(&world, opener), End::Closed);
    }
    let completions: u32 = openers.iter().map(|&opener| turns(&world, opener)).sum();
    assert!(world.stats().provider_calls > completions, "some calls were retried");
}

#[test]
fn a_session_fails_once_its_retries_run_out() {
    let calm = Settings::calm(5);
    let settings = Settings { provider: Config { overloaded: 1000, ..calm.provider }, ..calm };
    let mut world = World::new(settings);
    let opener = world.submit(Time::ZERO, spec(b"fix the build"));
    world.run(ITERATIONS);

    assert_eq!(end(&world, opener), End::Failed { failure: Failure::Overloaded });
    assert_eq!(world.stats().calls, calm.agent.retries + 1);
}

#[test]
fn calls_slower_than_their_timeout_time_out_and_their_late_answers_are_dropped() {
    let calm = Settings::calm(6);
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
    let opener = world.submit(Time::ZERO, spec(b"fix the build"));
    world.run(ITERATIONS);

    assert_eq!(end(&world, opener), End::Failed { failure: Failure::TimedOut });
    let stats = world.stats();
    assert_eq!((stats.calls, stats.timeouts, stats.late_answers), (3, 3, 3));
}

#[test]
fn an_expiring_session_cancels_its_call_in_flight() {
    let calm = Settings::calm(7);
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
    let opener = world.submit(Time::ZERO, spec(b"fix the build"));
    world.run(ITERATIONS);

    assert_eq!(end(&world, opener), End::Expired);
    let stats = world.stats();
    assert_eq!((stats.cancels, stats.late_answers), (1, 1));
}

#[test]
fn an_expiring_session_cancels_its_tool_in_flight() {
    let calm = Settings::calm(8);
    let settings = Settings {
        agent: Limits { session_timeout: Duration::from_secs(10), ..calm.agent },
        tool: Span::millis(60_000, 60_000),
        ..calm
    };
    let mut world = World::new(settings);
    let opener = world.submit(Time::ZERO, spec(b"fix the build"));
    world.run(ITERATIONS);

    assert_eq!((end(&world, opener), turns(&world, opener)), (End::Expired, 1));
    assert_eq!(world.stats().tool_cancels, 1);
}

#[test]
fn a_session_left_yielded_expires_and_its_openers_late_close_is_dropped() {
    let calm = Settings::calm(9);
    let settings = Settings {
        agent: Limits { session_timeout: Duration::from_secs(60), ..calm.agent },
        think: Span::millis(3_600_000, 3_600_000),
        ..calm
    };
    let mut world = World::new(settings);
    let opener = world.submit(Time::ZERO, spec(b"fix the build"));
    world.run(ITERATIONS);

    assert_eq!(yields(&world, opener), [(Yield::Done, &b"done"[..])]);
    assert_eq!(end(&world, opener), End::Expired);
    let stats = world.stats();
    assert_eq!((stats.closes, stats.stale), (1, 1));
}

#[test]
fn an_opener_that_closes_a_calling_session_has_its_call_cancelled() {
    let calm = Settings::calm(10);
    let settings = Settings { abandon: 1000, abandon_after: Span::millis(0, 0), ..calm };
    let mut world = World::new(settings);
    let opener = world.submit(Time::ZERO, spec(b"fix the build"));
    world.run(ITERATIONS);

    assert_eq!((end(&world, opener), turns(&world, opener)), (End::Closed, 0));
    assert!(yields(&world, opener).is_empty());
    let stats = world.stats();
    assert_eq!((stats.cancels, stats.late_answers), (1, 1));
}

#[test]
fn an_opener_that_closes_a_tooling_session_has_its_tool_cancelled() {
    let calm = Settings::calm(11);
    let settings = Settings {
        tool: Span::millis(60_000, 60_000),
        abandon: 1000,
        abandon_after: Span::millis(10_000, 10_000),
        ..calm
    };
    let mut world = World::new(settings);
    let opener = world.submit(Time::ZERO, spec(b"fix the build"));
    world.run(ITERATIONS);

    assert_eq!((end(&world, opener), turns(&world, opener)), (End::Closed, 1));
    assert_eq!(world.stats().tool_cancels, 1);
}

#[test]
fn the_turn_limit_ends_a_session_that_keeps_calling_tools() {
    let calm = Settings::calm(12);
    let settings = Settings {
        agent: Limits { turns: 3, ..calm.agent },
        provider: Config { tool_rounds: 100, ..calm.provider },
        ..calm
    };
    let mut world = World::new(settings);
    let opener = world.submit(Time::ZERO, spec(b"fix the build"));
    world.run(ITERATIONS);

    assert_eq!((end(&world, opener), turns(&world, opener)), (End::TurnLimit, 3));
}

#[test]
fn a_seed_replays_to_the_same_run() {
    let replay = |seed| {
        let mut world = World::new(noisy(seed));
        submit_noisily(&mut world, seed);
        world.run(ITERATIONS);
        (world.trace().to_vec(), world.stats(), world.now())
    };
    let (trace, stats, end) = replay(13);
    assert!(trace.len() > 20, "the run did something");
    assert_eq!(replay(13), (trace.clone(), stats, end));
    assert_ne!(replay(14).0, trace);
}

/// Hundreds of worlds with random limits, faults, schedules and openers: each
/// settles, with every session ended once and nothing left alive or in flight
/// (checked by `World::run`), and between them they reach every way a session
/// can yield and end.
#[test]
fn random_worlds_settle_with_every_session_ended() {
    let mut ends = BTreeSet::new();
    let mut stops = BTreeSet::new();
    let mut stale = 0;
    for seed in 0..300 {
        let mut world = World::new(noisy(seed));
        submit_noisily(&mut world, seed);
        world.run(ITERATIONS);
        stale += world.stats().stale;
        for (_, session) in world.sessions() {
            for (stop, _) in &session.yields {
                stops.insert(format!("{stop:?}"));
            }
            let kind = match session.ended.expect("every session ends").end {
                End::Busy => "busy",
                End::Invalid => "invalid",
                End::Closed => "closed",
                End::Failed { failure: Failure::TimedOut } => "timed out",
                End::Failed { .. } => "failed",
                End::Expired => "expired",
                End::TurnLimit => "turn limit",
                End::TranscriptFull => "transcript full",
            };
            ends.insert(kind);
        }
    }
    let expected = ["busy", "closed", "expired", "failed", "timed out", "transcript full", "turn limit"];
    assert_eq!(ends, expected.into_iter().collect());
    assert_eq!(stops, ["Done", "Malformed", "Refused"].into_iter().map(String::from).collect());
    assert!(stale > 0, "some continues and closes reached sessions that had ended");
}

/// Settings drawn from `seed`: small limits, faults, latencies that race the
/// deadlines, and an opener that nudges, dawdles and abandons.
fn noisy(seed: u64) -> Settings {
    let mut rng = Rng::new(seed);
    let calm = Settings::calm(seed);
    let mut millis = |low: u64, high: u64| Duration::from_millis(rng.between(low, high));
    let call_timeout = millis(500, 5_000);
    let session_timeout = millis(2_000, 120_000);
    let latency_max = millis(10, 4_000);
    let think = millis(0, 5_000);
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
            refused: pick(0, 100),
            no_calls: pick(0, 100),
            tool_rounds: pick(0, 5),
            ..calm.provider
        },
        tool: Span::millis(10, 3_000),
        tool_errors: pick(0, 300),
        nudges: Count { min: 0, max: pick(0, 3) },
        think: Span { min: Duration::ZERO, max: think },
        abandon: pick(0, 400),
        abandon_after: Span::millis(0, 30_000),
        ..calm
    }
}

/// Up to eight sessions opened at random times in the first minute.
fn submit_noisily(world: &mut World, seed: u64) {
    let mut rng = Rng::new(seed.wrapping_add(2));
    for _ in 0..rng.between(1, 8) {
        let at = Time::from_nanos(rng.between(0, 60_000_000_000));
        world.submit(at, spec(b"make the tests pass"));
    }
}
