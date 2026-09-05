// Authoritative root-pivot recovery regressions.

#[test]
fn three_nonviable_roots_stop_without_graph_or_conventional_authority() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    let third_root = "00000000-0000-4000-8000-000000000003";
    for id in ["first-root", "second-root", "third-root"] {
        state.on_tool_dispatched(&call(id, "codebase_memory_search_graph"), 0);
    }
    let roots = [ROOT, OTHER_ROOT, third_root].map(|root| {
        output_with_kinds(
            "codebase_memory_search_graph",
            root,
            DecisionAnchorLineageStageV1::Root,
            &[DecisionAnchorTargetKindV1::FunctionName],
        )
    });
    assert_eq!(
        state.on_tool_batch_finished(&[
            ("first-root", "codebase_memory_search_graph", &roots[0]),
            ("second-root", "codebase_memory_search_graph", &roots[1]),
            ("third-root", "codebase_memory_search_graph", &roots[2]),
        ]),
        DecisionAnchorTransition::Unchanged,
    );
    for (turn, id, expected) in [
        (1, "broad-one", DecisionAnchorTransition::Unchanged),
        (
            2,
            "broad-two",
            DecisionAnchorTransition::EnabledEvidenceIncomplete,
        ),
    ] {
        state.on_tool_dispatched(&call(id, "codebase_memory_get_architecture"), turn);
        assert_eq!(
            state.on_tool_finished(id, "codebase_memory_get_architecture", &plain_success()),
            expected,
        );
    }

    assert_eq!(state.recovery_details(), None);
    assert_eq!(
        state.on_tool_dispatched(&call("closed-graph", "codebase_memory_search_graph"), 3),
        exhausted_graph_denial(all_missing()),
    );
    let target = TargetAdmissionOutcome::Eligible(exact_target(TARGET_A));
    let mutation = call("incomplete-mutation", "write");
    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &mutation,
            4,
            Some(&mutation_targets(vec![target.clone()])),
        ),
        Some(ToolCallDenial::DecisionAnchorMutation),
    );
    let read = call("incomplete-source-read", "read");
    assert_eq!(
        state.on_tool_dispatched_with_targets(&read, 5, Some(&read_target(TARGET_A))),
        None,
    );
    state.on_tool_finished("incomplete-source-read", "read", &successful_read());
    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &mutation,
            6,
            Some(&mutation_targets(vec![target])),
        ),
        Some(ToolCallDenial::DecisionAnchorMutation),
    );
}
#[test]
fn nonviable_roots_pivot_once_each_then_stop_without_oscillation() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    let root_kinds = [
        DecisionAnchorTargetKindV1::FunctionName,
        DecisionAnchorTargetKindV1::QualifiedName,
    ];
    for id in ["first-root", "second-root"] {
        state.on_tool_dispatched(&call(id, "codebase_memory_search_graph"), 0);
    }
    let first = output_with_kinds(
        "codebase_memory_search_graph",
        ROOT,
        DecisionAnchorLineageStageV1::Root,
        &root_kinds,
    );
    let second = output_with_kinds(
        "codebase_memory_search_graph",
        OTHER_ROOT,
        DecisionAnchorLineageStageV1::Root,
        &root_kinds,
    );
    state.on_tool_batch_finished(&[
        ("first-root", "codebase_memory_search_graph", &first),
        ("second-root", "codebase_memory_search_graph", &second),
    ]);
    for (turn, id, expected) in [
        (1, "broad-one", DecisionAnchorTransition::Unchanged),
        (2, "broad-two", DecisionAnchorTransition::GapRecoveryNeeded),
    ] {
        state.on_tool_dispatched(&call(id, "codebase_memory_get_architecture"), turn);
        assert_eq!(
            state.on_tool_finished(id, "codebase_memory_get_architecture", &plain_success()),
            expected,
        );
    }

    assert_eq!(
        state.recovery_details().unwrap().compatible_actions,
        [GraphRecoveryActionV1::for_evidence(
            GraphRecoveryEvidenceKindV1::Implementation,
        )],
    );
    state.on_tool_dispatched(
        &source_call("first-implementation", DecisionEvidenceKindV1::Implementation),
        3,
    );
    assert_eq!(
        finish_with_evidence(
            &mut state,
            "first-implementation",
            ROOT,
            DecisionEvidenceKindV1::Implementation,
        ),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );
    state.on_tool_dispatched(&call("first-empty-trace", "codebase_memory_trace_path"), 4);
    assert_eq!(
        state.on_tool_finished(
            "first-empty-trace",
            "codebase_memory_trace_path",
            &output_with_caller_discovery(
                ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
                CallerDiscoveryOutcomeV1::NoEligibleSelector,
            ),
        ),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );
    let pivot = state.recovery_details().expect("sibling recovery route");
    assert_eq!(pivot.missing_evidence, all_missing());
    assert_eq!(
        pivot.compatible_actions,
        [GraphRecoveryActionV1::for_evidence(
            GraphRecoveryEvidenceKindV1::Implementation,
        )],
    );

    state.on_tool_dispatched(
        &source_call("second-implementation", DecisionEvidenceKindV1::Implementation),
        5,
    );
    finish_with_evidence(
        &mut state,
        "second-implementation",
        OTHER_ROOT,
        DecisionEvidenceKindV1::Implementation,
    );
    state.on_tool_dispatched(&call("second-empty-trace", "codebase_memory_trace_path"), 6);
    assert_eq!(
        state.on_tool_finished(
            "second-empty-trace",
            "codebase_memory_trace_path",
            &output_with_caller_discovery(
                OTHER_ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
                CallerDiscoveryOutcomeV1::NoEligibleSelector,
            ),
        ),
        DecisionAnchorTransition::EnabledEvidenceIncomplete,
    );
    assert!(state.blocks_mutation("write"));
}
