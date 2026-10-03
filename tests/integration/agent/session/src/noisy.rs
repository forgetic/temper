//! Random settings and sessions: small limits, a provider that fails and
//! answers slowly or badly, tools that fail, openers that abandon, cancel
//! and close twice; and sessions opened at random, with budgets of every
//! size, some too large and some spent before they start.

use temper_agent_model_session::{Budget, Limits, Spec};
use temper_agent_model_tools as tools;
use temper_lib::{Duration, Rng, Time};
use temper_llm_model::Config;

use crate::{BUDGET, Count, Settings, Span, World, spec};

/// Settings drawn from `seed`: small limits, faults, latencies that race the
/// deadlines, and an opener that nudges, dawdles and abandons.
#[must_use]
pub fn noisy(seed: u64) -> Settings {
    let mut rng = Rng::new(seed);
    let calm = Settings::calm(seed);
    let mut millis = |low: u64, high: u64| Duration::from_millis(rng.between(low, high));
    let call_timeout = millis(500, 5_000);
    let tool_timeout = millis(500, 5_000);
    let session_timeout = millis(2_000, 120_000);
    let latency_max = millis(10, 4_000);
    let think = millis(0, 5_000);
    let mut rng = Rng::new(seed.wrapping_add(1));
    let mut pick = |low: u64, high: u64| u32::try_from(rng.between(low, high)).expect("small numbers");
    let (sessions, parallel_tools) = (pick(1, 4), pick(1, 4));
    Settings {
        agent: Limits {
            sessions,
            messages: pick(4, 16),
            session_bytes: u64::from(pick(2_000, 8_000)),
            budget: Budget { turns: pick(1, 6), time: session_timeout, ..BUDGET },
            retries: pick(0, 4),
            backoff_base: Duration::from_millis(50),
            backoff_max: Duration::from_secs(2),
            call_timeout,
            tool_timeout,
            facts: pick(0, 24),
            parallel_tools,
            tools: tools::Limits {
                kits: sessions,
                calls: parallel_tools + pick(0, 2),
                known_files: pick(1, 4),
                file_timeout: millis(200, 5_000),
                facts: pick(0, 24),
                ..calm.agent.tools
            },
            ..calm.agent
        },
        provider: Config {
            calls: pick(1, 8),
            latency_min: Duration::from_millis(10),
            latency_max,
            overloaded: pick(0, 300),
            rate_limited: pick(0, 200),
            unavailable: pick(0, 100),
            too_long: pick(0, 20),
            unauthorized: pick(0, 10),
            refused: pick(0, 100),
            no_calls: pick(0, 100),
            answer_tokens: pick(1, 40),
            calls_per_answer: pick(1, 3),
            malformed: pick(0, 200),
            tool_rounds: pick(0, 5),
            ..calm.provider
        },
        tool: Span::millis(10, 3_000),
        tool_errors: pick(0, 300),
        nudges: Count { min: 0, max: pick(0, 3) },
        think: Span { min: Duration::ZERO, max: think },
        abandon: pick(0, 400),
        abandon_after: Span::millis(0, 30_000),
        cancels_lost: pick(0, 600),
        double_close: pick(0, 500),
        serve: pick(0, 800),
        granule: Duration::from_millis((pick(0, 1) * pick(100, 1_000)).into()),
        serving: Span::millis(10, 3_000),
        ..calm
    }
}

/// Up to eight sessions opened at random times in the first minute, each with
/// a budget of its own, small enough to run out in any dimension, now and then
/// empty in one, and now and then more than the limits allow; and room for
/// short answers only.
pub fn submit_noisily(world: &mut World, settings: &Settings, seed: u64) {
    let mut rng = Rng::new(seed.wrapping_add(2));
    let most = settings.agent.budget;
    for _ in 0..rng.between(1, 8) {
        let at = Time::from_nanos(rng.between(0, 60_000_000_000));
        let turns = if rng.chance(50) { most.turns + 1 } else { most.turns };
        let mut budget = Budget {
            turns: u32::try_from(rng.between(1, turns.into())).expect("a small number"),
            input: rng.between(5, 80),
            output: rng.between(5, 150),
            cache_read: rng.between(10, 600),
            cache_write: rng.between(5, 80),
            time: Duration::from_nanos(rng.between(1_000_000_000, most.time.as_nanos())),
        };
        if rng.chance(60) {
            match rng.below(6) {
                0 => budget.turns = 0,
                1 => budget.input = 0,
                2 => budget.output = 0,
                3 => budget.cache_read = 0,
                4 => budget.cache_write = 0,
                _ => budget.time = Duration::ZERO,
            }
        }
        // Answers short enough that some are cut, a tool call included.
        let max_tokens = u32::try_from(rng.between(4, 64)).expect("a small number");
        world.submit(at, Spec { budget, max_tokens, ..spec(b"make the tests pass") });
    }
}
