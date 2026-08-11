#[allow(dead_code)]
#[path = "support/opaque_decision_chain.rs"]
mod opaque_decision_chain;

use opaque_decision_chain::{DecisionCase, DecisionStep, run};

#[test]
fn jig_agent_does_not_mutate_after_an_unrelated_later_turn_target() {
    let run = run(DecisionCase::UnrelatedLaterTarget);

    assert_eq!(
        run.mutation, None,
        "a successful producer followed by an unrelated dependent target must not reach mutation"
    );
    assert_eq!(
        run.steps,
        vec![
            DecisionStep::Discovery,
            DecisionStep::UnrelatedLaterTarget,
            DecisionStep::MutationAttempt,
            DecisionStep::MutationBlocked,
            DecisionStep::Complete,
        ],
        "a merely successful tool sequence is not consumed, result-derived evidence"
    );
}

#[test]
fn jig_agent_does_not_mutate_after_dependent_reads_in_the_producer_turn() {
    let run = run(DecisionCase::ProducerTurnDependents);

    assert_eq!(
        run.mutation, None,
        "producer-turn refinement, trace, or source reads must not reach mutation"
    );
    assert_eq!(
        run.steps,
        vec![
            DecisionStep::ProducerTurnDependents,
            DecisionStep::MutationAttempt,
            DecisionStep::MutationBlocked,
            DecisionStep::Complete,
        ],
        "same-turn dependent reads cannot consume a producer result"
    );
}

#[test]
fn jig_agent_blocks_conventional_read_substitution_and_incomplete_source_evidence() {
    for case in [
        DecisionCase::ConventionalReadSubstitution,
        DecisionCase::IncompleteSourceEvidence,
    ] {
        let run = run(case);
        assert_eq!(run.mutation, None, "{case:?} must leave no mutation");
        assert!(
            run.steps.contains(&DecisionStep::MutationAttempt)
                && run.steps.contains(&DecisionStep::MutationBlocked),
            "{case:?} must reach the actual core mutation gate"
        );
    }
}

#[test]
fn jig_agent_uses_conventional_fallback_after_an_unavailable_expected_descendant() {
    let run = run(DecisionCase::UnavailableAfterRoot);

    assert_eq!(
        run.mutation,
        Some("conventional fallback after unavailable provider\n".to_string()),
        "the trusted unavailable result must release only conventional fallback"
    );
    assert_eq!(
        run.steps,
        vec![
            DecisionStep::Discovery,
            DecisionStep::Refinement,
            DecisionStep::UnavailableFallback,
            DecisionStep::Mutation,
            DecisionStep::Complete,
        ]
    );
}

#[test]
fn jig_agent_bounds_unconsumable_anchor_recovery_without_a_product() {
    let run = run(DecisionCase::UnconsumableRecoveryExhausted);

    assert_eq!(run.mutation, None, "recovery exhaustion must never mutate");
    assert_eq!(
        run.steps,
        vec![
            DecisionStep::Discovery,
            DecisionStep::Recovery,
            DecisionStep::Recovery,
        ],
        "the native agent gets two generic corrective attempts before safe termination"
    );
}
