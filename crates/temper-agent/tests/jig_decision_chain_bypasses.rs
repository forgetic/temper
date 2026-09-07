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
            DecisionStep::SourceRead,
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
fn jig_agent_requires_complete_evidence_or_an_exact_conventional_fallback() {
    for (case, expected) in [
        (
            DecisionCase::ImplementationOnlyProviderFallback,
            "independent conventional fallback completed\n",
        ),
        (
            DecisionCase::ImplementationFocusedProviderFallback,
            "implementation-focused fallback completed\n",
        ),
    ] {
        let run = run(case);
        assert_eq!(run.mutation.as_deref(), Some(expected));
        assert_eq!(
            run.steps,
            if case == DecisionCase::ImplementationOnlyProviderFallback {
                vec![
                    DecisionStep::Discovery,
                    DecisionStep::Refinement,
                    DecisionStep::ImplementationSource,
                    DecisionStep::ProviderFailure,
                    DecisionStep::MutationAttempt,
                    DecisionStep::MutationBlocked,
                    DecisionStep::SourceRead,
                    DecisionStep::Mutation,
                    DecisionStep::Complete,
                ]
            } else {
                vec![
                    DecisionStep::Discovery,
                    DecisionStep::Refinement,
                    DecisionStep::ImplementationSource,
                    DecisionStep::Trace,
                    DecisionStep::CallerSource,
                    DecisionStep::SourceRead,
                    DecisionStep::ProviderFailure,
                    DecisionStep::MutationAttempt,
                    DecisionStep::MutationBlocked,
                    DecisionStep::SourceRead,
                    DecisionStep::Mutation,
                    DecisionStep::Complete,
                ]
            },
        );
    }
}

#[test]
fn jig_agent_stops_when_successful_targeted_results_retain_no_decision_evidence() {
    let run = run(DecisionCase::NoRetainedDecisionEvidence);

    assert_eq!(run.mutation.as_deref(), Some("pending exact read\n"));
    assert_eq!(
        run.steps,
        vec![
            DecisionStep::Discovery,
            DecisionStep::SourceRead,
            DecisionStep::MutationAttempt,
            DecisionStep::MutationBlocked,
            DecisionStep::Complete,
        ],
    );
}

#[test]
fn jig_agent_stops_after_exact_reads_for_every_incomplete_successful_kind_matrix() {
    for (case, expected_steps) in [
        (
            DecisionCase::ImplementationOnlyIncomplete,
            vec![
                DecisionStep::Discovery,
                DecisionStep::Refinement,
                DecisionStep::ImplementationSource,
                DecisionStep::SourceRead,
                DecisionStep::MutationAttempt,
                DecisionStep::MutationBlocked,
                DecisionStep::Complete,
            ],
        ),
        (
            DecisionCase::ImplementationAndFocusedTestIncomplete,
            vec![
                DecisionStep::Discovery,
                DecisionStep::Refinement,
                DecisionStep::ImplementationSource,
                DecisionStep::BehavioralTestSource,
                DecisionStep::SourceRead,
                DecisionStep::MutationAttempt,
                DecisionStep::MutationBlocked,
                DecisionStep::Complete,
            ],
        ),
        (
            DecisionCase::ImplementationAndCallerIncomplete,
            vec![
                DecisionStep::Discovery,
                DecisionStep::Refinement,
                DecisionStep::ImplementationSource,
                DecisionStep::Trace,
                DecisionStep::CallerSource,
                DecisionStep::SourceRead,
                DecisionStep::MutationAttempt,
                DecisionStep::MutationBlocked,
                DecisionStep::Complete,
            ],
        ),
    ] {
        let run = run(case);
        assert_eq!(
            run.mutation.as_deref(),
            Some("pending exact read\n"),
            "{case:?} changed the seeded target",
        );
        assert_eq!(run.steps, expected_steps, "{case:?}");
    }
}

#[test]
fn jig_agent_completes_a_root_coherent_forest_before_exact_read_and_mutation() {
    let run = run(DecisionCase::RootCoherentForest);

    assert_eq!(run.mutation.as_deref(), Some("verified forest evidence\n"));
    assert_eq!(
        run.steps,
        vec![
            DecisionStep::Discovery,
            DecisionStep::ImplementationSource,
            DecisionStep::Trace,
            DecisionStep::CallerSource,
            DecisionStep::BehavioralTestSource,
            DecisionStep::SourceRead,
            DecisionStep::Mutation,
            DecisionStep::Complete,
        ],
    );
    let report = run
        .report
        .expect("complete forest returns a workspace report");
    for private in [
        "PRIVATE-PROVIDER-PAYLOAD",
        "PRIVATE-PROVIDER-SOURCE",
        "Authorization: Bearer PRIVATE",
        "/srv/private/checkout",
        "crate::opaque_",
        "qualified_name",
        "decision_evidence_kind",
        "\"arguments\"",
    ] {
        assert!(
            !report.contains(private),
            "workspace summary leaked {private:?}"
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
    assert!(
        !run.steps.contains(&DecisionStep::GraphRetry),
        "trusted unavailability must not trigger an immediate graph retry",
    );
}

#[test]
fn jig_agent_stops_after_every_graph_root_becomes_nonviable() {
    let run = run(DecisionCase::AllRootsNonViableIncomplete);

    assert_eq!(
        run.mutation,
        Some("pending exact read\n".to_string()),
        "incomplete enabled evidence must not release conventional mutation authority",
    );
    assert_eq!(
        run.steps,
        vec![
            DecisionStep::Discovery,
            DecisionStep::Recovery,
            DecisionStep::Recovery,
            DecisionStep::ImplementationSource,
            DecisionStep::Trace,
            DecisionStep::ImplementationSource,
            DecisionStep::Trace,
            DecisionStep::ImplementationSource,
            DecisionStep::Trace,
        ],
    );
}

#[test]
fn jig_agent_stops_when_a_viable_root_has_no_actionable_descendants() {
    let run = run(DecisionCase::SelectorlessViableRootIncomplete);

    assert_eq!(
        run.mutation,
        Some("pending exact read\n".to_string()),
        "selector-less enabled evidence must stop without conventional fallback",
    );
    assert_eq!(
        run.steps,
        vec![
            DecisionStep::Discovery,
            DecisionStep::ImplementationSource,
            DecisionStep::Trace,
        ],
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
