//! The agent's session child domain at random: many noisy worlds, each settled
//! with every session ended, every way a session ends reached among them.

use std::collections::BTreeSet;

use temper_agent_domain_session::End;
use temper_agent_domain_session::llm::Failure;
use temper_agent_session_world::{Ended, World, noisy, submit_noisily};

const ITERATIONS: u32 = 100_000;

/// A thousand worlds with random limits, faults, schedules and openers: each
/// settles, with every session ended once and nothing left alive or in flight
/// (checked by `World::run`), and between them they reach every way a session
/// can yield and end.
#[test]
fn random_worlds_settle_with_every_session_ended() {
    let mut ends = BTreeSet::new();
    let mut stops = BTreeSet::new();
    let (mut stale, mut invalid, mut not_run, mut parallel, mut op_timeouts) = (0, 0, 0, 0, 0);
    let mut most_runs = 0;
    let mut failures = BTreeSet::new();
    let mut races = [0; 4];
    let mut served = [0; 6];
    for seed in 0..1000 {
        let settings = noisy(seed);
        let mut world = World::new(settings);
        submit_noisily(&mut world, &settings, seed);
        world.run(ITERATIONS);
        stale += world.stats().stale;
        parallel = parallel.max(world.stats().most_parallel);
        most_runs = most_runs.max(world.stats().most_runs);
        op_timeouts += world.stats().op_timeouts;
        invalid += world.told().0.invalid_calls;
        not_run += world.stats().not_run;
        let stats = world.stats();
        let delegated = [
            stats.delegates,
            stats.withdraws,
            stats.answered_after_withdraw,
            stats.delegate_timeouts,
            stats.finishes_refused,
            stats.finishes_accepted,
        ];
        for (count, more) in served.iter_mut().zip(delegated) {
            *count += more;
        }
        let won = [
            stats.answered_after_cancel,
            stats.failed_after_cancel,
            stats.done_after_cancel,
            stats.closed_while_closing,
        ];
        for (race, count) in races.iter_mut().zip(won) {
            *race += count;
        }
        for (_, session) in world.sessions() {
            if let Some(Ended { end: End::Failed { failure }, .. }) = session.ended {
                failures.insert(format!("{failure:?}"));
            }
            for (stop, _) in &session.yields {
                stops.insert(format!("{stop:?}"));
            }
            let kind = match session.ended.expect("every session ends").end {
                End::Busy => "busy".into(),
                End::Invalid => "invalid".into(),
                End::Closed => "closed".into(),
                End::Failed { failure: Failure::TimedOut } => "timed out".into(),
                End::Failed { .. } => "failed".into(),
                End::Budget { spent } => format!("{spent:?}"),
                End::TranscriptFull => "transcript full".into(),
            };
            ends.insert(kind);
        }
    }
    let expected = [
        "busy",
        "invalid",
        "closed",
        "failed",
        "timed out",
        "transcript full",
        "Turns",
        "Input",
        "Output",
        "CacheRead",
        "CacheWrite",
        "Time",
    ];
    assert_eq!(ends, expected.into_iter().map(String::from).collect());
    assert_eq!(stops, ["Done", "Malformed", "Refused", "Truncated"].into_iter().map(String::from).collect());
    assert!(stale > 0, "some continues and closes reached sessions that had ended");
    assert!(parallel > 1, "some reads ran side by side");
    assert!(most_runs > parallel, "an iteration held more runs, ended ones among them, than a batch");
    assert!(
        served.iter().all(|&count| count > 0),
        "the opener served calls, some withdrawn, some answered all the same, some late, finishes refused and accepted: {served:?}"
    );
    assert!(
        races.iter().all(|&count| count > 0),
        "calls, failures and runs won races with cancels, and closes came twice: {races:?}"
    );
    for failure in ["Overloaded", "Unavailable", "ContextTooLong", "Unauthorized"] {
        assert!(failures.contains(failure), "some session failed as {failure}: {failures:?}");
    }
    assert!(op_timeouts > 0, "some operations of the tools ran out of time");
    assert!(
        invalid > 0 && not_run > 0,
        "some calls were malformed, and some were cut short and not run: {invalid} {not_run}"
    );
}
