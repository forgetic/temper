//! End to end at the session child domain: the agent's sessions, their opener
//! and a fake provider's domain, talking through a simulated world.

use temper_agent_domain_session::llm::Failure;
use temper_agent_domain_session::{Budget, Dimension, End, Limits, Spec, Yield};
use temper_agent_domain_session_tests::{
    BUDGET, Count, Ended, Settings, Span, Told, World, noisy, spec, submit_noisily,
};
use temper_agent_domain_tools::{self as tools, Authority, Grants};
use temper_lib::{Duration, Time};
use temper_llm_domain::Config;

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

/// The calm spec, with `budget`.
fn budgeted(budget: Budget) -> Spec {
    Spec { budget, ..spec(b"fix the build") }
}

fn out_of(spent: Dimension) -> End {
    End::Budget { spent }
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
    assert_eq!((stats.calls, stats.provider_calls, stats.results, stats.closes), (3, 3, 2, 1));
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
    assert_eq!((stats.continues, stats.results, stats.closes), (2, 6, 1));
}

#[test]
fn malformed_calls_are_answered_with_their_problem_and_the_conversation_goes_on() {
    let calm = Settings::calm(18);
    let settings = Settings { provider: Config { malformed: 1000, calls_per_answer: 3, ..calm.provider }, ..calm };
    let mut world = World::new(settings);
    let opener = world.submit(Time::ZERO, spec(b"fix the build"));
    world.run(ITERATIONS);

    // Every call is malformed: none runs, and each gets an answer, or the
    // provider would refuse the next query.
    assert_eq!(yields(&world, opener), [(Yield::Done, &b"done"[..])]);
    assert_eq!((world.stats().ops, world.stats().results), (0, 0));
    let (told, _) = world.told();
    assert!(told.invalid_calls >= 2 && told.invalid_calls == told.calls, "{told:?}");
}

/// The world checks every batch as it starts: no more reads at once than
/// the limits allow, a write alone, and the results back in call order.
#[test]
fn reads_in_one_answer_run_side_by_side_and_writes_alone() {
    let calm = Settings::calm(19);
    let settings = Settings {
        agent: Limits { parallel_tools: 3, ..calm.agent },
        provider: Config { calls_per_answer: 6, tool_rounds: 4, ..calm.provider },
        ..calm
    };
    let mut world = World::new(settings);
    let openers: Vec<u64> = (0..4).map(|_| world.submit(Time::ZERO, spec(b"fix the build"))).collect();
    world.run(ITERATIONS);

    for opener in openers {
        assert_eq!((end(&world, opener), turns(&world, opener)), (End::Closed, 5));
    }
    let (told, _) = world.told();
    assert_eq!(world.stats().results, told.calls, "each call's result went back, what comes of that call");
    assert_eq!(world.stats().most_parallel, 3, "reads ran as many at once as the limits allow");
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
fn a_session_out_of_time_cancels_its_call_in_flight() {
    let calm = Settings::calm(7);
    let settings = Settings {
        provider: Config {
            latency_min: Duration::from_secs(30),
            latency_max: Duration::from_secs(30),
            ..calm.provider
        },
        ..calm
    };
    let mut world = World::new(settings);
    let opener = world.submit(Time::ZERO, budgeted(Budget { time: Duration::from_secs(10), ..BUDGET }));
    world.run(ITERATIONS);

    assert_eq!(end(&world, opener), out_of(Dimension::Time));
    let stats = world.stats();
    assert_eq!((stats.cancels, stats.late_answers), (1, 1));
}

#[test]
fn a_session_out_of_time_cancels_its_tool_in_flight() {
    let calm = Settings::calm(9);
    let mut world = World::new(Settings { tool: Span::millis(60_000, 60_000), ..calm });
    let opener = world.submit(Time::ZERO, budgeted(Budget { time: Duration::from_secs(10), ..BUDGET }));
    world.run(ITERATIONS);

    assert_eq!((end(&world, opener), turns(&world, opener)), (out_of(Dimension::Time), 1));
    assert_eq!(world.stats().op_cancels, 1);
}

#[test]
fn a_session_left_yielded_runs_out_of_time_and_its_openers_late_close_is_dropped() {
    let calm = Settings::calm(9);
    let mut world = World::new(Settings { think: Span::millis(3_600_000, 3_600_000), ..calm });
    let opener = world.submit(Time::ZERO, budgeted(Budget { time: Duration::from_secs(60), ..BUDGET }));
    world.run(ITERATIONS);

    assert_eq!(yields(&world, opener), [(Yield::Done, &b"done"[..])]);
    assert_eq!(end(&world, opener), out_of(Dimension::Time));
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
    assert_eq!(world.stats().op_cancels, 1);
}

#[test]
fn a_tool_call_that_runs_out_of_time_goes_back_to_the_llm_and_the_conversation_goes_on() {
    let calm = Settings::calm(21);
    let settings = Settings {
        agent: Limits { tool_timeout: Duration::from_secs(1), ..calm.agent },
        tool: Span::millis(5_000, 5_000),
        ..calm
    };
    let mut world = World::new(settings);
    let opener = world.submit(Time::ZERO, spec(b"fix the build"));
    world.run(ITERATIONS);

    assert_eq!(yields(&world, opener), [(Yield::Done, &b"done"[..])]);
    assert_eq!((end(&world, opener), turns(&world, opener)), (End::Closed, 3));
    let stats = world.stats();
    assert_eq!((stats.results, stats.op_timeouts, stats.op_cancels), (2, 2, 0));
}

#[test]
fn a_call_that_wins_its_race_with_a_cancel_still_ends_the_session_and_counts() {
    let calm = Settings::calm(21);
    let settings = Settings { abandon: 1000, abandon_after: Span::millis(0, 0), cancels_lost: 1000, ..calm };
    let mut world = World::new(settings);
    let opener = world.submit(Time::ZERO, spec(b"fix the build"));
    world.run(ITERATIONS);

    // The answer came all the same: its tokens count, and nothing yields.
    assert_eq!((end(&world, opener), turns(&world, opener)), (End::Closed, 1));
    assert!(yields(&world, opener).is_empty());
    let stats = world.stats();
    assert_eq!((stats.cancels_lost, stats.answered_after_cancel, stats.cancels), (1, 1, 0));
}

#[test]
fn a_tool_run_that_wins_its_race_with_a_cancel_still_ends_the_session() {
    let calm = Settings::calm(22);
    let settings = Settings {
        tool: Span::millis(30_000, 30_000),
        abandon: 1000,
        abandon_after: Span::millis(10_000, 10_000),
        cancels_lost: 1000,
        ..calm
    };
    let mut world = World::new(settings);
    let opener = world.submit(Time::ZERO, spec(b"fix the build"));
    world.run(ITERATIONS);

    assert_eq!((end(&world, opener), turns(&world, opener)), (End::Closed, 1));
    let stats = world.stats();
    assert_eq!((stats.op_cancels_lost, stats.done_after_cancel, stats.op_cancels), (1, 1, 0));
}

/// A spec that grants no tools of the session's own, so that the LLM calls
/// only those the opener serves.
fn served_only() -> Spec {
    let spec = spec(b"fix the build");
    let grants = Grants { inspect: false, modify: false, shell: false };
    Spec { authority: Authority { grants, ..spec.authority }, ..spec }
}

#[test]
fn a_finish_is_refused_once_then_accepted_and_the_opener_closes_the_session() {
    let calm = Settings::calm(23);
    let settings = Settings { serve: 1000, provider: Config { tool_rounds: 20, ..calm.provider }, ..calm };
    let mut world = World::new(settings);
    let opener = world.submit(Time::ZERO, served_only());
    world.run(ITERATIONS);

    assert_eq!(end(&world, opener), End::Closed);
    let stats = world.stats();
    assert_eq!((stats.finishes_refused, stats.finishes_accepted), (1, 1));
    assert!(stats.delegates >= 2 && stats.ops == 0 && stats.results == 0, "{stats:?}");
}

#[test]
fn an_answer_races_its_withdraw() {
    for (lost, withdrawn, answered) in [(1000, 0, 1), (0, 1, 0)] {
        let calm = Settings::calm(24);
        let settings = Settings {
            serve: 1000,
            serving: Span::millis(30_000, 30_000),
            abandon: 1000,
            abandon_after: Span::millis(5_000, 5_000),
            cancels_lost: lost,
            ..calm
        };
        let mut world = World::new(settings);
        let opener = world.submit(Time::ZERO, served_only());
        world.run(ITERATIONS);

        assert_eq!((end(&world, opener), turns(&world, opener)), (End::Closed, 1));
        let stats = world.stats();
        assert_eq!((stats.withdraws, stats.answered_after_withdraw), (withdrawn, answered));
    }
}

/// A close and a run's end that come at the same instant, the close first:
/// the cancel the close sends goes out after the run has ended, and loses.
#[test]
fn a_cancel_sent_after_its_run_ended_in_the_same_iteration_is_a_lost_race() {
    let mut crossed = 0;
    for seed in 0..40 {
        let calm = Settings::calm(seed);
        let settings = Settings {
            granule: Duration::from_secs(5),
            tool: Span::millis(1, 10_000),
            serve: 500,
            serving: Span::millis(1, 10_000),
            abandon: 1000,
            abandon_after: Span::millis(0, 30_000),
            ..calm
        };
        let mut world = World::new(settings);
        for _ in 0..3 {
            world.submit(Time::ZERO, spec(b"fix the build"));
        }
        world.run(ITERATIONS);
        crossed += world.stats().cancels_crossed;
    }
    assert!(crossed > 0, "some cancels crossed the end of their run");
}

#[test]
fn the_turn_budget_ends_a_session_that_keeps_calling_tools() {
    let calm = Settings::calm(12);
    let mut world = World::new(Settings { provider: Config { tool_rounds: 100, ..calm.provider }, ..calm });
    let opener = world.submit(Time::ZERO, budgeted(Budget { turns: 3, ..BUDGET }));
    world.run(ITERATIONS);

    // The tools of the last turn run; their results do not go back.
    assert_eq!((end(&world, opener), turns(&world, opener)), (out_of(Dimension::Turns), 3));
    let (told, _) = world.told();
    assert_eq!((world.stats().results, told.calls, told.tools_answered), (2, 3, 3));
}

#[test]
fn the_output_budget_cuts_the_last_answer_short_and_ends_the_session() {
    let calm = Settings::calm(15);
    let settings =
        Settings { provider: Config { answer_tokens: 50, ..calm.provider }, nudges: Count { min: 5, max: 5 }, ..calm };
    let mut world = World::new(settings);
    // Two tool calls take eight tokens each; the answer gets at most four, and
    // the nudges whatever is left.
    let opener = world.submit(Time::ZERO, budgeted(Budget { output: 20, ..BUDGET }));
    world.run(ITERATIONS);

    assert_eq!(end(&world, opener), out_of(Dimension::Output));
    assert_eq!(yields(&world, opener).last(), Some(&(Yield::Truncated, &b"do"[..])));
    assert_eq!(world.session(opener).usage.output_tokens, 20, "no more than the budget");
}

#[test]
fn the_token_budgets_end_a_session_after_the_turn_that_uses_them_up() {
    let calm = Settings::calm(16);
    let settings = Settings { nudges: Count { min: 100, max: 100 }, ..calm };
    let budgets = [
        (Budget { input: 30, ..BUDGET }, Dimension::Input),
        (Budget { cache_read: 100, ..BUDGET }, Dimension::CacheRead),
        (Budget { cache_write: 30, ..BUDGET }, Dimension::CacheWrite),
    ];
    for (budget, spent) in budgets {
        let mut world = World::new(settings);
        let opener = world.submit(Time::ZERO, budgeted(budget));
        world.run(ITERATIONS);
        assert_eq!(end(&world, opener), out_of(spent), "{budget:?}");
        assert!(turns(&world, opener) > 1, "{budget:?} pays for some turns");
    }
}

#[test]
fn a_spec_that_asks_for_more_than_the_limits_is_refused() {
    let mut world = World::new(Settings::calm(17));
    let opener = world.submit(Time::ZERO, budgeted(Budget { turns: BUDGET.turns + 1, ..BUDGET }));
    world.run(ITERATIONS);
    assert_eq!(end(&world, opener), End::Invalid);
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

/// Facts are told on the side: sessions that keep none of them make the same
/// requests at the same times as sessions that keep them all, and the facts
/// kept add up to what crossed the boundary (checked by `World::run`).
#[test]
fn facts_change_nothing_the_sessions_do() {
    for seed in 0..100 {
        let run = |facts| {
            let noisy = noisy(seed);
            let tools = tools::Limits { facts, ..noisy.agent.tools };
            let settings = Settings { agent: Limits { facts, tools, ..noisy.agent }, ..noisy };
            let mut world = World::new(settings);
            submit_noisily(&mut world, &settings, seed);
            world.run(ITERATIONS);
            (world.trace().to_vec(), world.stats(), world.told())
        };
        let (trace, stats, (told, lost)) = run(4096);
        assert_eq!(lost, 0, "seed {seed}: room for every fact");
        assert!(told.ended > 0, "seed {seed}: facts were told");
        let (silent, same, (none, dropped)) = run(0);
        assert!(silent == trace && same == stats, "seed {seed}: the same requests, at the same times");
        assert_eq!((none, dropped > 0), (Told::default(), true), "seed {seed}: every fact dropped");
    }
}
