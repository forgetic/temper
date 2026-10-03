//! Random settings: small limits, charters that sometimes do not fit them,
//! faults, cancels, and latencies that race the deadlines.

use temper_agent_domain_run::{Budget, Limits};
use temper_lib::{Duration, Rng};

use crate::partner::Script;
use crate::{Checkouts, Settings, Span, host};

/// Settings drawn from `seed`: small limits, charters that sometimes do not fit
/// them, faults, cancels, and latencies that race the deadlines.
#[must_use]
pub fn noisy(seed: u64) -> Settings {
    let mut rng = Rng::new(seed);
    let calm = Settings::calm(seed);
    let mut pick = |low: u64, high: u64| rng.between(low, high);
    let small = |n: u64| u32::try_from(n).expect("small numbers");
    let run = Limits {
        runs: small(pick(1, 4)),
        conversations: small(pick(1, 4)),
        run_bytes: pick(1_000, 6_000),
        nudges: small(pick(0, 3)),
        budget: Budget { turns: 30, time: Duration::from_secs(3600), ..calm.run.budget },
        ..calm.run
    };
    let host = host::Script {
        jobs: small(pick(1, 8)),
        window: Duration::from_secs(pick(0, 120)),
        cancels: small(pick(0, 300)),
        cancel: Span { min: Duration::ZERO, max: Duration::from_secs(pick(1, 300)) },
        brief_min: 0,
        brief_max: small(pick(100, 4_000)),
        turns_min: 1,
        turns_max: small(pick(5, 35)),
        tokens_min: 500,
        tokens_max: pick(1_000, 200_000),
        time: Span { min: Duration::from_secs(5), max: Duration::from_secs(pick(60, 4_000)) },
        ..calm.host
    };
    let host = host::Script {
        recancels: small(pick(0, 300)),
        late_cancels: small(pick(0, 300)),
        push: Span::millis(0, pick(0, 3_000)),
        moved: small(pick(0, 300)),
        push_failures: small(pick(0, 200)),
        ..host
    };
    let partner = Script {
        conversations: small(pick(0, 4)),
        invalid: small(pick(0, 30)),
        turn: Span::millis(10, pick(100, 20_000)),
        input: pick(1, 4_000),
        output: pick(1, 1_000),
        cache: pick(0, 2_000),
        faults: small(pick(0, 100)),
        finishes: small(pick(0, 300)),
        asks: small(pick(0, 300)),
        bad_asks: small(pick(0, 300)),
        shares: small(pick(0, 500)),
        parallel: small(pick(1, 4)),
        changes: small(pick(0, 1000)),
        good: small(pick(0, 1000)),
        yields: small(pick(0, 500)),
        odd_stops: small(pick(0, 300)),
        settle: Span::millis(0, pick(0, 2_000)),
        races: small(pick(0, 1000)),
    };
    let checkout = Checkouts {
        guides: small(pick(0, 1000)),
        guide_max: small(pick(1, 3000)),
        checks: small(pick(0, 1000)),
        check_failures: small(pick(0, 3)),
        not_text: small(pick(0, 300)),
        io: Span::millis(0, pick(0, 6_000)),
        io_failures: small(pick(0, 200)),
        check: Span::millis(0, pick(0, 20_000)),
    };
    let run = Limits { guide_bytes: small(pick(1, 2000)), io_timeout: Duration::from_secs(pick(1, 5)), ..run };
    let run = Limits {
        outcome_bytes: pick(10, 400),
        check_timeout: Duration::from_millis(pick(1_000, 30_000)),
        check_tail: small(pick(0, 300)),
        ..run
    };
    let inject = small(pick(0, 150));
    // Sub-agents: room for a few, nested a little.
    let run = Limits {
        conversations: run.conversations.saturating_mul(small(pick(1, 3))),
        calls: small(pick(1, 8)),
        depth: small(pick(0, 3)),
        run_conversations: small(pick(1, 4)),
        answer_bytes: small(pick(0, 200)),
        facts: small(pick(0, 64)),
        ..run
    };
    let host = host::Script { agents: small(pick(0, 1000)), ..host };
    Settings {
        run,
        host,
        partner,
        hop: Span::millis(0, pick(0, 50)),
        checkout,
        races: small(pick(0, 1000)),
        inject,
        ..calm
    }
}
