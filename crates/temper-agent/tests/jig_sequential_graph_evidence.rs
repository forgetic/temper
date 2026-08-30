#[allow(dead_code)]
#[path = "support/opaque_decision_chain.rs"]
mod opaque_decision_chain;

use opaque_decision_chain::{DecisionCase, DecisionStep, run};

#[test]
fn jig_agent_consumes_opaque_result_driven_evidence_before_mutation() {
    let run = run(DecisionCase::Consumed);
    assert_eq!(
        run.mutation,
        Some("verified evidence\n".to_string()),
        "only the consumed decision chain may mutate"
    );
    assert_eq!(
        run.steps,
        vec![
            DecisionStep::Discovery,
            DecisionStep::Refinement,
            DecisionStep::ImplementationSource,
            DecisionStep::Trace,
            DecisionStep::CallerSource,
            DecisionStep::FocusedTestSearch,
            DecisionStep::BehavioralTestSource,
            DecisionStep::Mutation,
            DecisionStep::Complete,
        ],
        "opaque provider results must drive later-turn dependent targets before mutation"
    );
}

#[test]
fn jig_agent_recovers_missing_caller_and_rejects_repeated_focused_test_detours() {
    let run = run(DecisionCase::StagedDetourRecovery);
    assert_eq!(run.mutation, Some("staged detours recovered\n".to_string()));
    assert_eq!(
        run.steps,
        vec![
            DecisionStep::Discovery,
            DecisionStep::Refinement,
            DecisionStep::ImplementationSource,
            DecisionStep::Trace,
            DecisionStep::CallerSourceDetour,
            DecisionStep::CallerSource,
            DecisionStep::FocusedTestDetour,
            DecisionStep::FocusedTestDetour,
            DecisionStep::FocusedTestSearch,
            DecisionStep::BehavioralTestSource,
            DecisionStep::SourceRead,
            DecisionStep::Mutation,
            DecisionStep::Complete,
        ],
        "local detours must preserve one exact implementation/caller/semantic-test chain",
    );
}
