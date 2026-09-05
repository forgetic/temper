fn correction_trace_output() -> ToolOutput {
    let mut output = output(
        "codebase_memory_trace_path",
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
    );
    let details = output.details.as_mut().unwrap();
    let lineage: DecisionAnchorLineageV1 = serde_json::from_value(
        details[SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY].clone(),
    )
    .unwrap();
    details[SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY] = serde_json::to_value(
        lineage
            .with_implementation_correction_available()
            .unwrap(),
    )
    .unwrap();
    output
}

fn complete_state_with_correction() -> DecisionAnchorState {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    state.on_tool_dispatched(&call("root", "codebase_memory_search_graph"), 0);
    finish(
        &mut state,
        "root",
        "codebase_memory_search_graph",
        ROOT,
        DecisionAnchorLineageStageV1::Root,
    );
    state.on_tool_dispatched(
        &source_call("implementation", DecisionEvidenceKindV1::Implementation),
        1,
    );
    state.on_tool_finished_with_source_target(
        "implementation",
        "codebase_memory_get_code_snippet",
        &output_with_evidence(
            ROOT,
            DecisionAnchorLineageStageV1::CarryForward,
            DecisionEvidenceKindV1::Implementation,
        ),
        Some(&TargetAdmissionOutcome::Eligible(exact_target(TARGET_A))),
    );
    state.on_tool_dispatched(&call("trace", "codebase_memory_trace_path"), 2);
    state.on_tool_finished("trace", "codebase_memory_trace_path", &correction_trace_output());
    state.on_tool_dispatched(
        &source_call("caller", DecisionEvidenceKindV1::Caller),
        3,
    );
    finish_with_evidence(&mut state, "caller", ROOT, DecisionEvidenceKindV1::Caller);
    state.on_tool_dispatched(
        &source_call("focused", DecisionEvidenceKindV1::FocusedTest),
        4,
    );
    assert_eq!(
        finish_with_evidence(
            &mut state,
            "focused",
            ROOT,
            DecisionEvidenceKindV1::FocusedTest,
        ),
        DecisionAnchorTransition::EnabledEvidenceComplete,
    );
    assert_eq!(
        state.active_recovery_action().map(|(_, action)| action),
        Some(GraphRecoveryActionV1::implementation_authority_correction()),
    );
    state
}

#[test]
fn ordinary_read_and_same_batch_read_close_pre_mutation_correction() {
    let correction_call = source_call("correction", DecisionEvidenceKindV1::Implementation);
    let correction_admission = LineageAdmissionOutcome::Eligible(
        EligibleLineageAdmission::implementation_authority_correction(ROOT.to_string(), false)
            .unwrap(),
    );

    let mut after_read = complete_state_with_correction();
    assert_eq!(
        after_read.on_tool_dispatched_with_targets(
            &call("ordinary-read", "read"),
            5,
            Some(&read_target(TARGET_A)),
        ),
        None,
    );
    assert_eq!(after_read.active_recovery_action(), None);
    assert_eq!(
        after_read.on_tool_dispatched_with_admission(
            &correction_call,
            6,
            Some(&correction_admission),
        ),
        completed_graph_denial(),
        "an exercised ordinary read closes correction before provider dispatch",
    );

    let mut same_batch = complete_state_with_correction();
    assert_eq!(
        same_batch.on_tool_batch_dispatched_with_admissions_and_targets(
            &[correction_call, call("same-batch-read", "read")],
            5,
            &[Some(correction_admission), None],
            &[None, Some(read_target(TARGET_A))],
        ),
        [completed_graph_denial(), None],
        "a correction cannot race an ordinary read from the same immutable batch",
    );
    assert_eq!(same_batch.active_recovery_action(), None);
}

