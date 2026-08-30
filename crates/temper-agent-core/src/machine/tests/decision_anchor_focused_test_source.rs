use super::*;

#[test]
fn fallback_source_without_typed_test_evidence_exhausts_without_retry() {
    let mut state = focused_test_recovery_state();
    assert_eq!(
        state.on_tool_dispatched_with_admission(
            &semantic_fallback_call("semantic-fallback"),
            6,
            Some(&LineageAdmissionOutcome::Eligible(fallback_admission())),
        ),
        None,
    );
    assert_eq!(
        state.on_tool_finished(
            "semantic-fallback",
            "codebase_memory_search_graph",
            &output_with_focused_test_discovery(
                "codebase_memory_search_graph",
                ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
                FocusedTestDiscoveryOutcomeV1::EligibleSelectorReturned,
            ),
        ),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );
    assert_eq!(
        state.on_tool_dispatched_with_admission(
            &source_call("non-test", DecisionEvidenceKindV1::FocusedTest),
            7,
            Some(&LineageAdmissionOutcome::Eligible(exact_test_admission())),
        ),
        None,
    );
    assert_eq!(
        state.on_tool_finished(
            "non-test",
            "codebase_memory_get_code_snippet",
            &output_with_kinds(
                "codebase_memory_get_code_snippet",
                ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
                &[DecisionAnchorTargetKindV1::QualifiedName],
            ),
        ),
        DecisionAnchorTransition::EnabledEvidenceIncomplete,
    );
    assert!(state.blocks_mutation("write"));
    assert_eq!(
        state.on_tool_dispatched(
            &source_call("retry", DecisionEvidenceKindV1::FocusedTest),
            8,
        ),
        exhausted_graph_denial([GraphRecoveryEvidenceKindV1::FocusedTest]),
    );
}
