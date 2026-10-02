//! End to end at the run sub-model: runs started by a fake worker's model, with
//! a scripted partner playing their conversations, in a simulated world.
//!

use std::collections::{BTreeMap, BTreeSet};

use temper_agent_model_run::{Answer, Budget, Exhausted, Failure, Fault, Invalid, Limits, Policy, Refusal};
use temper_agent_model_run_tests::partner::Script;
use temper_agent_model_run_tests::{Checkouts, Settings, Span, World};
use temper_fake_worker_model::Config;
use temper_lib::{Duration, Rng};

const ITERATIONS: u32 = 1_000_000;

fn settled(settings: &Settings) -> World {
    let mut world = World::new(*settings);
    world.run(ITERATIONS);
    world
}

fn answers(world: &World) -> Vec<&Answer> {
    world.answers().map(|answer| answer.expect("every start is answered")).collect()
}

fn failure(answer: &Answer) -> Failure {
    match answer {
        Answer::Failed { failure, .. } => *failure,
        Answer::Refused(refusal) => panic!("the run was refused: {refusal:?}"),
        Answer::Accepted { outcome, .. } => panic!("the run finished with {outcome:?}"),
    }
}

#[test]
fn an_llm_that_keeps_stopping_is_nudged_until_the_run_fails_as_unfinished() {
    let calm = Settings::calm(1);
    let world = settled(&Settings { partner: Script { yields: 1000, ..calm.partner }, ..calm });
    for answer in answers(&world) {
        assert_eq!(failure(answer), Failure::Policy(Policy::Unfinished { nudges: calm.run.nudges, rejected: 0 }));
    }
    let stats = world.stats();
    assert_eq!((stats.opens, stats.says, stats.closes), (4, 4 * calm.run.nudges, 4));
    assert_eq!(stats.partner.nudged, 4 * calm.run.nudges);
}

#[test]
fn an_llm_that_fails_fails_its_run_with_the_fault() {
    let calm = Settings::calm(2);
    let world = settled(&Settings { partner: Script { faults: 1000, ..calm.partner }, ..calm });
    for answer in answers(&world) {
        assert!(
            [Failure::Model(Fault::Provider), Failure::Model(Fault::ContextFull)].contains(&failure(answer)),
            "{answer:?}"
        );
    }
    assert_eq!(world.stats().closes, 0, "the conversations ended on their own");
}

#[test]
fn an_llm_whose_last_stop_shows_a_fault_fails_the_run_with_it() {
    let calm = Settings::calm(3);
    let script = Script { yields: 1000, odd_stops: 1000, ..calm.partner };
    let world = settled(&Settings { run: Limits { nudges: 0, ..calm.run }, partner: script, ..calm });
    let faults = [Fault::Truncated, Fault::Refused, Fault::Malformed].map(Failure::Model);
    for answer in answers(&world) {
        assert!(faults.contains(&failure(answer)), "{answer:?}");
    }
}

#[test]
fn an_llm_that_works_on_runs_out_of_turns() {
    let calm = Settings::calm(4);
    let world = settled(&Settings { partner: Script { yields: 0, ..calm.partner }, ..calm });
    for answer in answers(&world) {
        assert_eq!(failure(answer), Failure::Budget(Exhausted::Turns));
    }
    assert!(world.stats().partner.ceilings >= 4, "each conversation kept to its share of turns");
}

#[test]
fn an_llm_that_spends_past_the_tokens_fails_its_run_for_budget() {
    let calm = Settings::calm(5);
    let worker = Config { tokens_min: 5_000, tokens_max: 20_000, ..calm.worker };
    let world = settled(&Settings { worker, partner: Script { yields: 0, ..calm.partner }, ..calm });
    let tokens =
        [Exhausted::Input, Exhausted::Output, Exhausted::CacheRead, Exhausted::CacheWrite].map(Failure::Budget);
    for answer in answers(&world) {
        assert!(tokens.contains(&failure(answer)), "{answer:?}");
    }
}

#[test]
fn a_run_out_of_time_fails_for_time() {
    let calm = Settings::calm(6);
    let worker = Config { time_min: Duration::from_secs(10), time_max: Duration::from_secs(20), ..calm.worker };
    let script = Script { turn: Span::millis(3_000, 8_000), yields: 0, ..calm.partner };
    let world = settled(&Settings { worker, partner: script, ..calm });
    for answer in answers(&world) {
        assert_eq!(failure(answer), Failure::Budget(Exhausted::Time));
    }
}

#[test]
fn a_cancelled_run_closes_its_conversation_and_answers_as_cancelled() {
    let calm = Settings::calm(7);
    let worker =
        Config { cancels: 1000, cancel_min: Duration::ZERO, cancel_max: Duration::from_secs(30), ..calm.worker };
    let world = settled(&Settings { worker, partner: Script { yields: 0, ..calm.partner }, ..calm });
    for answer in answers(&world) {
        assert_eq!(failure(answer), Failure::Cancelled);
    }
    let stats = world.stats();
    assert_eq!((stats.cancels, stats.closes), (4, 4));
}

#[test]
fn a_cancel_that_comes_before_main_starts_closes_main_once_it_does() {
    let calm = Settings::calm(8);
    let worker = Config { cancels: 1000, cancel_min: Duration::ZERO, cancel_max: Duration::ZERO, ..calm.worker };
    // The checkout is read at once, and the cancel is back within two trips to
    // the worker; main's start takes two hops longer than that.
    let checkout = Checkouts { io: Span::millis(0, 0), ..calm.checkout };
    let world = settled(&Settings { worker, hop: Span::millis(50, 100), checkout, ..calm });
    for answer in answers(&world) {
        assert_eq!(failure(answer), Failure::Cancelled);
    }
    let first = |what: &str| world.trace().iter().position(|line| line.contains(what)).expect("it happened");
    assert!(first("run <- Cancel {") < first("run <- Started {"), "the first cancel came before the first start");
    let stats = world.stats();
    assert_eq!((stats.cancels, stats.closes, stats.partner.closed), (4, 4, 4));
}

#[test]
fn a_cancel_while_the_run_reads_its_checkout_answers_once_the_read_has_ended() {
    let calm = Settings::calm(13);
    let worker = Config { cancels: 1000, cancel_min: Duration::ZERO, cancel_max: Duration::ZERO, ..calm.worker };
    let checkout = Checkouts { io: Span::millis(500, 1_000), ..calm.checkout };
    let world = settled(&Settings { worker, checkout, ..calm });
    for answer in answers(&world) {
        assert_eq!(failure(answer), Failure::Cancelled);
    }
    let stats = world.stats();
    assert_eq!((stats.cancels, stats.opens, stats.reads), (4, 0, 4), "each run stopped at its first read");
}

#[test]
fn a_run_reads_its_checkouts_guides_and_looks_for_checks() {
    let calm = Settings::calm(14);
    let checkout = Checkouts { guides: 1000, checks: 1000, ..calm.checkout };
    let world = settled(&Settings { partner: Script { yields: 1000, ..calm.partner }, checkout, ..calm });
    let stats = world.stats();
    assert_eq!(stats.opens, 4);
    assert!(stats.reads >= 4 && stats.probes > 0, "{stats:?}");
}

#[test]
fn starts_beyond_the_run_slots_are_refused_as_busy() {
    let calm = Settings::calm(8);
    let settings = Settings {
        run: Limits { runs: 1, ..calm.run },
        worker: Config { window: Duration::ZERO, ..calm.worker },
        partner: Script { yields: 1000, ..calm.partner },
        ..calm
    };
    let world = settled(&settings);
    let busy = answers(&world).into_iter().filter(|answer| **answer == Answer::Refused(Refusal::Busy)).count();
    assert_eq!(busy, 3);
}

#[test]
fn charters_beyond_the_limits_are_refused_as_invalid() {
    let calm = Settings::calm(9);
    let run = Limits { budget: Budget { turns: 10, ..calm.run.budget }, ..calm.run };
    let world = settled(&Settings { run, ..calm });
    for answer in answers(&world) {
        assert_eq!(answer, &Answer::Refused(Refusal::Invalid(Invalid::Budget)));
    }
    assert_eq!(world.stats().opens, 0);
}

#[test]
fn a_main_conversation_refused_at_its_entrance_refuses_its_run() {
    let calm = Settings::calm(10);
    let world = settled(&Settings { partner: Script { conversations: 0, ..calm.partner }, ..calm });
    for answer in answers(&world) {
        assert_eq!(answer, &Answer::Refused(Refusal::Busy));
    }
    let world = settled(&Settings { partner: Script { invalid: 1000, ..calm.partner }, ..calm });
    for answer in answers(&world) {
        assert_eq!(answer, &Answer::Refused(Refusal::Invalid(Invalid::Conversation)));
    }
}

/// A world where the LLM finishes, with outcomes that fit, and changes land
/// after their checks fail a time or two and their branch moves once.
fn finishing(seed: u64) -> Settings {
    let calm = Settings::calm(seed);
    Settings {
        worker: Config { moves: 1, writable: 1000, changes: 1000, checks: 1000, verdicts: 1000, ..calm.worker },
        partner: Script { finishes: 300, yields: 0, ..calm.partner },
        checkout: Checkouts { checks: 1000, check_failures: 2, ..calm.checkout },
        ..calm
    }
}

#[test]
fn an_llm_that_finishes_with_what_fits_has_its_run_accepted() {
    let world = settled(&finishing(15));
    for answer in answers(&world) {
        assert!(matches!(answer, Answer::Accepted { .. }), "{answer:?}");
    }
    assert_eq!(world.stats().partner.accepted, 4);
}

#[test]
fn changes_land_after_their_checks_pass_and_their_push_goes_through() {
    let settings = finishing(16);
    let world = settled(&Settings { partner: Script { changes: 1000, ..settings.partner }, ..settings });
    let stats = world.stats();
    assert_eq!(stats.partner.accepted, 4, "{stats:?}");
    assert!(stats.partner.checks_failed > 0 && stats.partner.moved > 0, "{stats:?}");
    assert!(world.trace().iter().any(|line| line.contains("Checking {")), "the worker hears of each check");
}

#[test]
fn an_outcome_that_does_not_fit_is_rejected_and_the_llm_tries_again() {
    let settings = finishing(17);
    let world = settled(&Settings { partner: Script { good: 300, changes: 0, ..settings.partner }, ..settings });
    let stats = world.stats();
    assert!(stats.partner.rejected > 0, "{stats:?}");
    for answer in answers(&world) {
        assert!(matches!(answer, Answer::Accepted { .. } | Answer::Failed { .. }), "{answer:?}");
    }
}

#[test]
fn a_finish_past_its_deadline_is_withdrawn_and_its_checks_aborted() {
    let settings = finishing(18);
    let world = settled(&Settings {
        partner: Script { changes: 1000, finish_deadline: Span::millis(1_000, 3_000), ..settings.partner },
        checkout: Checkouts { check: Span::millis(500, 6_000), ..settings.checkout },
        races: 0,
        ..settings
    });
    let stats = world.stats();
    assert!(stats.partner.withdrawn > 0 && stats.aborts > 0, "{stats:?}");
    assert!(stats.partner.cancelled > 0, "{stats:?}");
}

#[test]
fn a_seed_replays_to_the_same_run() {
    let replay = |seed| {
        let world = settled(&noisy(seed));
        (world.trace().to_vec(), world.stats(), world.now())
    };
    let (trace, stats, end) = replay(12);
    assert!(trace.len() > 50, "the runs did something");
    assert_eq!(replay(12), (trace.clone(), stats, end));
    assert_ne!(replay(13).0, trace);
}

/// Hundreds of worlds with random limits, scripts, faults and schedules: each
/// settles with its invariants holding (checked by `World::run`), and between
/// them they reach every way a run can end in this batch.
#[test]
fn random_worlds_settle_with_every_start_answered_once() {
    let mut seen = BTreeSet::new();
    let (mut cancels, mut races, mut stale, mut expired) = (0, 0, 0, 0);
    for seed in 0..300 {
        let world = settled(&noisy(seed));
        let stats = world.stats();
        cancels += stats.cancels;
        races += stats.partner.races;
        stale += stats.partner.stale;
        expired += stats.partner.expired;
        for answer in answers(&world) {
            let kind = match answer {
                Answer::Accepted { .. } => "accepted",
                Answer::Refused(Refusal::Busy) => "busy",
                Answer::Refused(Refusal::Invalid(Invalid::Conversation)) => "conversation invalid",
                Answer::Refused(Refusal::Invalid(_)) => "invalid",
                Answer::Failed { failure, .. } => match failure {
                    Failure::Model(Fault::Provider | Fault::ContextFull) => "fault",
                    Failure::Model(Fault::Truncated | Fault::Refused | Fault::Malformed) => "stopped",
                    Failure::Budget(Exhausted::Turns) => "turns",
                    Failure::Budget(Exhausted::Time) => "time",
                    Failure::Budget(_) => "tokens",
                    Failure::Policy(Policy::Unfinished { .. }) => "unfinished",
                    Failure::Cancelled => "cancelled",
                },
            };
            seen.insert(kind);
        }
    }
    let mut expected = vec![
        "accepted",
        "busy",
        "cancelled",
        "conversation invalid",
        "fault",
        "invalid",
        "stopped",
        "time",
        "tokens",
        "turns",
        "unfinished",
    ];
    expected.sort_unstable();
    assert_eq!(seen.into_iter().collect::<Vec<_>>(), expected);
    // And the races: turns spent after a close, a close or nudge crossing a
    // conversation's own end, conversations out of time, cancels.
    assert!(cancels > 0 && races > 0 && stale > 0 && expired > 0, "{cancels} {races} {stale} {expired}");
}

/// Settings drawn from `seed`: small limits, charters that sometimes do not fit
/// them, faults, cancels, and latencies that race the deadlines.
fn noisy(seed: u64) -> Settings {
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
    let worker = Config {
        jobs: small(pick(1, 8)),
        window: Duration::from_secs(pick(0, 120)),
        cancels: small(pick(0, 300)),
        cancel_min: Duration::ZERO,
        cancel_max: Duration::from_secs(pick(1, 300)),
        brief_min: 0,
        brief_max: small(pick(100, 4_000)),
        turns_min: 1,
        turns_max: small(pick(5, 35)),
        tokens_min: 500,
        tokens_max: pick(1_000, 200_000),
        time_min: Duration::from_secs(5),
        time_max: Duration::from_secs(pick(60, 4_000)),
        ..calm.worker
    };
    let worker = Config {
        recancels: small(pick(0, 300)),
        late_cancels: small(pick(0, 300)),
        push_min: Duration::ZERO,
        push_max: Duration::from_millis(pick(0, 3_000)),
        moves: small(pick(0, 2)),
        push_failures: small(pick(0, 200)),
        ..worker
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
        changes: small(pick(0, 1000)),
        good: small(pick(0, 1000)),
        finish_deadline: Span::millis(100, pick(100, 30_000)),
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
    Settings {
        run,
        worker,
        partner,
        hop: Span::millis(0, pick(0, 50)),
        checkout,
        races: small(pick(0, 1000)),
        inject,
        ..calm
    }
}

/// Adds what `world` counted of the states cancels and deadlines found runs in.
fn tally(world: &World, cancels: &mut BTreeMap<&'static str, u32>, deadlines: &mut BTreeMap<&'static str, u32>) {
    for (cell, count) in world.cancel_cells() {
        *cancels.entry(*cell).or_insert(0) += count;
    }
    for (cell, count) in world.deadline_cells() {
        *deadlines.entry(*cell).or_insert(0) += count;
    }
}

/// Cancels at random moments, from the worker (once, twice, late) and handed
/// to the run right before or after any of its events, find runs in every
/// state, and each answers once (checked by `World::run`).
#[test]
fn cancels_find_runs_in_every_state() {
    let (mut cancels, mut deadlines) = (BTreeMap::new(), BTreeMap::new());
    for seed in 0..200 {
        tally(&settled(&noisy(seed)), &mut cancels, &mut deadlines);
    }
    // Cancelled twice while the run reads a slow checkout.
    let calm = Settings::calm(20);
    let worker = Config {
        cancels: 1000,
        cancel_min: Duration::ZERO,
        cancel_max: Duration::from_secs(1),
        recancels: 1000,
        ..calm.worker
    };
    let checkout = Checkouts { io: Span::millis(3_000, 5_000), ..calm.checkout };
    tally(&settled(&Settings { worker, checkout, ..calm }), &mut cancels, &mut deadlines);
    // Cancelled as changes are checked and pushed, and as the LLM spends past
    // its budget.
    for seed in 21..24 {
        let landing = finishing(seed);
        let run = Limits { runs: 16, conversations: 16, ..landing.run };
        let worker = Config {
            jobs: 16,
            cancels: 1000,
            cancel_min: Duration::from_secs(2),
            cancel_max: Duration::from_secs(30),
            tokens_min: 5_000,
            tokens_max: 40_000,
            ..landing.worker
        };
        let partner = Script { changes: 1000, ..landing.partner };
        let checkout = Checkouts { check: Span::millis(2_000, 8_000), check_failures: 3, ..landing.checkout };
        let settings = Settings { run, worker, partner, checkout, inject: 20, ..landing };
        tally(&settled(&settings), &mut cancels, &mut deadlines);
    }
    let cells = ["preparing", "stopping", "opening", "working", "landing", "over", "winding", "answered", "gone"];
    for cell in cells {
        assert!(cancels.get(cell).is_some_and(|count| *count > 0), "no cancel found a run {cell}: {cancels:?}");
    }
}

/// Deadlines short against a slow checkout and slow conversations fall due in
/// every state a run's deadline runs in.
#[test]
fn deadlines_find_runs_in_every_state_they_run_in() {
    let (mut cancels, mut deadlines) = (BTreeMap::new(), BTreeMap::new());
    for seed in 0..50 {
        let calm = Settings::calm(seed);
        let worker = Config { time_min: Duration::from_secs(1), time_max: Duration::from_secs(6), ..calm.worker };
        let checkout = Checkouts { io: Span::millis(0, 2_000), ..calm.checkout };
        let partner = Script { turn: Span::millis(100, 2_000), ..calm.partner };
        let settings = Settings { worker, checkout, partner, hop: Span::millis(0, 1_500), ..calm };
        tally(&settled(&settings), &mut cancels, &mut deadlines);
    }
    for cell in ["preparing", "opening", "working"] {
        assert!(deadlines.get(cell).is_some_and(|count| *count > 0), "no deadline found a run {cell}: {deadlines:?}");
    }
}
