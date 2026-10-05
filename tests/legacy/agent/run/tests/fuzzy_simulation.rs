//! The agent's run child domain at random: many noisy worlds, each settled with
//! every start answered once, every way a run ends reached among them.

use std::collections::BTreeSet;

use temper_legacy_agent_domain_run::{Answer, Exhausted, Failure, Fault, Invalid, Policy, Refusal};
use temper_legacy_agent_run_world::{Settings, World, noisy};

const ITERATIONS: u32 = 1_000_000;

fn settled(settings: &Settings) -> World {
    let mut world = World::new(*settings);
    world.run(ITERATIONS);
    world
}

fn answers(world: &World) -> Vec<&Answer> {
    world.answers().map(|answer| answer.expect("every start is answered")).collect()
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
                    Failure::Model(Fault::Provider | Fault::ContextFull | Fault::Exhausted) => "fault",
                    Failure::Model(Fault::Truncated | Fault::Refused | Fault::Malformed) => "stopped",
                    Failure::Budget(Exhausted::Turns) => "turns",
                    Failure::Budget(Exhausted::Time) => "time",
                    Failure::Budget(_) => "tokens",
                    Failure::Policy(Policy::Unfinished { .. }) => "unfinished",
                    Failure::Cancelled => "cancelled",
                    Failure::Stale => "stale",
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
        "stale",
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
