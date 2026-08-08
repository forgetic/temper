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
            DecisionStep::BypassStopped,
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
            DecisionStep::BypassStopped,
            DecisionStep::Complete,
        ],
        "same-turn dependent reads cannot consume a producer result"
    );
}
