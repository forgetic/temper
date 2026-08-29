// Authoritative root-pivot recovery regressions.

#[test]
fn three_nonviable_roots_release_fallback_without_graph_or_cross_root_authority() {
    let mut effects = effects();
    effects.insert("bash".to_string(), ToolEffects::process());
    effects.insert("submit_for_pr".to_string(), ToolEffects::process());
    let mut state = DecisionAnchorState::from_effects(&effects).unwrap();
    let third_root = "00000000-0000-4000-8000-000000000003";
    for id in ["first-root", "second-root", "third-root"] {
        state.on_tool_dispatched(&call(id, "codebase_memory_search_graph"), 0);
    }
    let roots = [ROOT, OTHER_ROOT, third_root]
        .map(|root| {
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
            DecisionAnchorTransition::ConventionalFallbackReleased,
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
        conventional_fallback_graph_denial(),
    );
    let target = TargetAdmissionOutcome::Eligible(exact_target(TARGET_A));
    let mutation = call("fallback-mutation", "write");
    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &mutation,
            4,
            Some(&mutation_targets(vec![target.clone()])),
        ),
        Some(ToolCallDenial::DecisionAnchorMutation),
    );
    let read = call("fallback-source-read", "read");
    assert_eq!(
        state.on_tool_dispatched_with_targets(&read, 5, Some(&read_target(TARGET_A))),
        None,
    );
    state.on_tool_finished("fallback-source-read", "read", &successful_read());
    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &mutation,
            6,
            Some(&mutation_targets(vec![target])),
        ),
        None,
    );
    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &call("validation", "bash"),
            7,
            Some(&InvocationTargetAdmission::SourceNeutralProcess),
        ),
        None,
    );
    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &call("submission", "submit_for_pr"),
            8,
            Some(&InvocationTargetAdmission::ControlPlane),
        ),
        None,
    );
}

#[test]
fn nonviable_roots_pivot_once_each_then_release_fallback_without_oscillation() {
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
    assert_eq!(
        state.on_tool_batch_finished(&[
            ("first-root", "codebase_memory_search_graph", &first),
            ("second-root", "codebase_memory_search_graph", &second),
        ]),
        DecisionAnchorTransition::Unchanged,
    );
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
            GraphRecoveryEvidenceKindV1::Trace,
        )],
    );
    state.on_tool_dispatched(&call("first-empty-trace", "codebase_memory_trace_path"), 3);
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
    state.on_tool_dispatched(
        &source_call("first-implementation", DecisionEvidenceKindV1::Implementation),
        4,
    );
    assert_eq!(
        finish_with_evidence(
            &mut state,
            "first-implementation",
            ROOT,
            DecisionEvidenceKindV1::Implementation,
        ),
        DecisionAnchorTransition::GapRecoveryNeeded,
        "the first nonviable root pivots instead of exhausting the forest",
    );
    let pivot = state.recovery_details().expect("sibling recovery route");
    assert_eq!(pivot.missing_evidence, all_missing());
    assert_eq!(
        pivot.compatible_actions,
        [GraphRecoveryActionV1::for_evidence(
            GraphRecoveryEvidenceKindV1::Trace,
        )]
    );
    assert_eq!(
        pivot.remaining_allowance,
        temper_protocol_activity::MAX_GRAPH_RECOVERY_ALLOWANCE_V1
    );

    state.on_tool_dispatched(&call("second-empty-trace", "codebase_memory_trace_path"), 5);
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
        DecisionAnchorTransition::GapRecoveryNeeded,
    );
    state.on_tool_dispatched(
        &source_call("second-implementation", DecisionEvidenceKindV1::Implementation),
        6,
    );
    assert_eq!(
        finish_with_evidence(
            &mut state,
            "second-implementation",
            OTHER_ROOT,
            DecisionEvidenceKindV1::Implementation,
        ),
        DecisionAnchorTransition::ConventionalFallbackReleased,
    );
    assert_eq!(
        state.on_tool_dispatched(&call("no-root-retry", "codebase_memory_trace_path"), 7),
        conventional_fallback_graph_denial(),
    );
}

#[test]
fn pivoted_root_rebuilds_evidence_before_exact_read_mutation_and_submission() {
    let mut effects = effects();
    effects.insert("bash".to_string(), ToolEffects::process());
    effects.insert("submit_for_pr".to_string(), ToolEffects::process());
    let mut state = DecisionAnchorState::from_effects(&effects).unwrap();
    let target = TargetAdmissionOutcome::Eligible(exact_target(TARGET_A));

    for id in ["stalled-root", "viable-root"] {
        state.on_tool_dispatched(&call(id, "codebase_memory_search_graph"), 0);
    }
    let stalled = output_with_focused_test_discovery(
        "codebase_memory_search_graph",
        ROOT,
        DecisionAnchorLineageStageV1::Root,
        FocusedTestDiscoveryOutcomeV1::NoEligibleSelector,
    );
    let viable = output_with_focused_test_discovery(
        "codebase_memory_search_graph",
        OTHER_ROOT,
        DecisionAnchorLineageStageV1::Root,
        FocusedTestDiscoveryOutcomeV1::EligibleSelectorReturned,
    );
    assert_eq!(
        state.on_tool_batch_finished(&[
            ("stalled-root", "codebase_memory_search_graph", &stalled),
            ("viable-root", "codebase_memory_search_graph", &viable),
        ]),
        DecisionAnchorTransition::Unchanged,
    );

    state.on_tool_dispatched(
        &source_call("old-implementation", DecisionEvidenceKindV1::Implementation),
        1,
    );
    assert_eq!(
        state.on_tool_finished_with_source_target(
            "old-implementation",
            "codebase_memory_get_code_snippet",
            &output_with_evidence(
                ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
                DecisionEvidenceKindV1::Implementation,
            ),
            Some(&target),
        ),
        DecisionAnchorTransition::Unchanged,
    );
    state.on_tool_dispatched(&call("old-trace", "codebase_memory_trace_path"), 2);
    assert_eq!(
        state.on_tool_finished(
            "old-trace",
            "codebase_memory_trace_path",
            &output_with_caller_discovery(
                ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
                CallerDiscoveryOutcomeV1::EligibleSelectorReturned,
            ),
        ),
        DecisionAnchorTransition::Unchanged,
    );
    state.on_tool_dispatched(
        &source_call("old-caller", DecisionEvidenceKindV1::Caller),
        3,
    );
    assert_eq!(
        finish_with_evidence(
            &mut state,
            "old-caller",
            ROOT,
            DecisionEvidenceKindV1::Caller,
        ),
        DecisionAnchorTransition::Unchanged,
    );
    for (turn, id, expected) in [
        (4, "broad-one", DecisionAnchorTransition::Unchanged),
        (5, "broad-two", DecisionAnchorTransition::GapRecoveryNeeded),
    ] {
        state.on_tool_dispatched(&call(id, "codebase_memory_get_architecture"), turn);
        assert_eq!(
            state.on_tool_finished(id, "codebase_memory_get_architecture", &plain_success()),
            expected,
        );
    }

    assert_eq!(
        finish_semantic_test_search(
            &mut state,
            "old-focused-search",
            ROOT,
            6,
            FocusedTestDiscoveryOutcomeV1::NoEligibleSelector,
        ),
        DecisionAnchorTransition::GapRecoveryNeeded,
        "the retained viable sibling must be selected before terminal exhaustion",
    );
    let pivot = state.recovery_details().expect("pivoted sibling recovery");
    assert_eq!(pivot.missing_evidence, all_missing());
    assert_eq!(
        pivot.remaining_allowance,
        temper_protocol_activity::MAX_GRAPH_RECOVERY_ALLOWANCE_V1
    );
    assert_eq!(
        pivot.compatible_actions,
        [GraphRecoveryActionV1::for_evidence(
            GraphRecoveryEvidenceKindV1::Trace,
        )],
    );
    for guidance in state.take_model_guidance() {
        for private in [ROOT, OTHER_ROOT, TARGET_A, "provider-derived focused test route"] {
            assert!(!guidance.contains(private));
        }
    }

    let old_root_read = call("old-root-read", "read");
    state.on_tool_dispatched_with_targets(&old_root_read, 8, Some(&read_target(TARGET_A)));
    state.on_tool_finished("old-root-read", "read", &successful_read());

    state.on_tool_dispatched(&call("new-trace", "codebase_memory_trace_path"), 9);
    assert_eq!(
        state.on_tool_finished(
            "new-trace",
            "codebase_memory_trace_path",
            &output_with_caller_discovery(
                OTHER_ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
                CallerDiscoveryOutcomeV1::EligibleSelectorReturned,
            ),
        ),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );
    state.on_tool_dispatched(
        &source_call("new-implementation", DecisionEvidenceKindV1::Implementation),
        10,
    );
    assert_eq!(
        state.on_tool_finished_with_source_target(
            "new-implementation",
            "codebase_memory_get_code_snippet",
            &output_with_evidence(
                OTHER_ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
                DecisionEvidenceKindV1::Implementation,
            ),
            Some(&target),
        ),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );
    for (turn, id, kind, expected) in [
        (
            11,
            "new-caller",
            DecisionEvidenceKindV1::Caller,
            DecisionAnchorTransition::GapRecoveryNeeded,
        ),
        (
            12,
            "new-focused-test",
            DecisionEvidenceKindV1::FocusedTest,
            DecisionAnchorTransition::Converged,
        ),
    ] {
        state.on_tool_dispatched(&source_call(id, kind), turn);
        assert_eq!(
            finish_with_evidence(&mut state, id, OTHER_ROOT, kind),
            expected,
        );
    }

    let mutation = call("matching-mutation", "write");
    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &mutation,
            13,
            Some(&mutation_targets(vec![target.clone()])),
        ),
        Some(ToolCallDenial::DecisionAnchorMutation),
        "the old root's exact read cannot authorize the independently completed root",
    );
    let exact_read = call("new-root-exact-read", "read");
    assert_eq!(
        state.on_tool_dispatched_with_targets(&exact_read, 14, Some(&read_target(TARGET_A))),
        None,
    );
    state.on_tool_finished("new-root-exact-read", "read", &successful_read());
    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &mutation,
            15,
            Some(&mutation_targets(vec![target])),
        ),
        None,
    );
    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &call("validation", "bash"),
            16,
            Some(&InvocationTargetAdmission::SourceNeutralProcess),
        ),
        None,
    );
    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &call("submission", "submit_for_pr"),
            17,
            Some(&InvocationTargetAdmission::ControlPlane),
        ),
        None,
    );
}
