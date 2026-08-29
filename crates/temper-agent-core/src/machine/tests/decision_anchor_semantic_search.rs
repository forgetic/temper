fn finish_semantic_test_search(
    state: &mut DecisionAnchorState,
    id: &str,
    root: &str,
    turn: usize,
    outcome: FocusedTestDiscoveryOutcomeV1,
) -> DecisionAnchorTransition {
    let mut search = call(id, "codebase_memory_search_graph");
    search.arguments = serde_json::json!({"query": "behavioral regression intent"});
    let admission = crate::EligibleLineageAdmission::focused_test_semantic_fallback(
        root.to_string(),
        DecisionAnchorTargetKindV1::GraphQuery,
    )
    .expect("semantic focused-test search admission");
    assert_eq!(
        state.on_tool_dispatched_with_admission(
            &search,
            turn,
            Some(&crate::LineageAdmissionOutcome::Eligible(admission)),
        ),
        None,
    );
    state.on_tool_finished(
        id,
        "codebase_memory_search_graph",
        &output_with_focused_test_discovery(
            "codebase_memory_search_graph",
            root,
            DecisionAnchorLineageStageV1::CarryForward,
            outcome,
        ),
    )
}

fn all_missing() -> [GraphRecoveryEvidenceKindV1; 4] {
    [
        GraphRecoveryEvidenceKindV1::Trace,
        GraphRecoveryEvidenceKindV1::Implementation,
        GraphRecoveryEvidenceKindV1::Caller,
        GraphRecoveryEvidenceKindV1::FocusedTest,
    ]
}
