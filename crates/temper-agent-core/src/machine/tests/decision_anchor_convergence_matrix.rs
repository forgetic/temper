// Complete and incomplete enabled decision-evidence convergence regressions.

fn state_with_kinds(kinds: &[DecisionEvidenceKindV1]) -> DecisionAnchorState {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    state.on_tool_dispatched(&call("matrix-root", "codebase_memory_search_graph"), 0);
    finish(
        &mut state,
        "matrix-root",
        "codebase_memory_search_graph",
        ROOT,
        DecisionAnchorLineageStageV1::Root,
    );

    if kinds.contains(&DecisionEvidenceKindV1::Implementation) {
        let target = TargetAdmissionOutcome::Eligible(exact_target(TARGET_A));
        state.on_tool_dispatched(
            &source_call("matrix-implementation", DecisionEvidenceKindV1::Implementation),
            1,
        );
        state.on_tool_finished_with_source_target(
            "matrix-implementation",
            "codebase_memory_get_code_snippet",
            &output_with_evidence(
                ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
                DecisionEvidenceKindV1::Implementation,
            ),
            Some(&target),
        );
    }
    if kinds.contains(&DecisionEvidenceKindV1::Caller) {
        state.on_tool_dispatched(&call("matrix-trace", "codebase_memory_trace_path"), 2);
        finish(
            &mut state,
            "matrix-trace",
            "codebase_memory_trace_path",
            ROOT,
            DecisionAnchorLineageStageV1::CarryForward,
        );
        state.on_tool_dispatched(
            &source_call("matrix-caller", DecisionEvidenceKindV1::Caller),
            3,
        );
        finish_with_evidence(
            &mut state,
            "matrix-caller",
            ROOT,
            DecisionEvidenceKindV1::Caller,
        );
    }
    if kinds.contains(&DecisionEvidenceKindV1::FocusedTest) {
        state.on_tool_dispatched(
            &source_call("matrix-focused", DecisionEvidenceKindV1::FocusedTest),
            4,
        );
        finish_with_evidence(
            &mut state,
            "matrix-focused",
            ROOT,
            DecisionEvidenceKindV1::FocusedTest,
        );
    }
    state
}

fn assert_exact_read_cannot_release_incomplete_authority(
    state: &mut DecisionAnchorState,
    case: &str,
) {
    let read = call("matrix-exact-read", "read");
    state.on_tool_dispatched_with_targets(&read, 10, Some(&read_target(TARGET_A)));
    state.on_tool_finished("matrix-exact-read", "read", &successful_read());

    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &call("matrix-mutation", "write"),
            11,
            Some(&mutation_targets(vec![TargetAdmissionOutcome::Eligible(
                exact_target(TARGET_A),
            )])),
        ),
        Some(ToolCallDenial::DecisionAnchorMutation),
        "{case} must not turn an ordinary exact read into enabled mutation authority",
    );
}

#[test]
fn every_successful_incomplete_kind_matrix_blocks_exact_read_mutation_authority() {
    let cases: &[(&str, &[DecisionEvidenceKindV1])] = &[
        ("no retained decision kinds", &[]),
        (
            "implementation only",
            &[DecisionEvidenceKindV1::Implementation],
        ),
        (
            "implementation and focused test without caller",
            &[
                DecisionEvidenceKindV1::Implementation,
                DecisionEvidenceKindV1::FocusedTest,
            ],
        ),
        (
            "implementation and caller without focused test",
            &[
                DecisionEvidenceKindV1::Implementation,
                DecisionEvidenceKindV1::Caller,
            ],
        ),
    ];

    for (case, kinds) in cases {
        let mut state = state_with_kinds(kinds);
        assert_exact_read_cannot_release_incomplete_authority(&mut state, case);
    }
}

#[test]
fn root_coherent_forest_requires_the_post_source_exact_read_before_mutation() {
    let mut guarded_effects = effects();
    guarded_effects.insert("bash".to_string(), ToolEffects::process());
    guarded_effects.insert("submit_for_pr".to_string(), ToolEffects::process());
    let mut state = DecisionAnchorState::from_effects(&guarded_effects).unwrap();
    for id in ["implementation-root", "focused-root"] {
        state.on_tool_dispatched(&call(id, "codebase_memory_search_graph"), 0);
    }
    let implementation_root = output(
        "codebase_memory_search_graph",
        ROOT,
        DecisionAnchorLineageStageV1::Root,
    );
    let focused_root = output(
        "codebase_memory_search_graph",
        OTHER_ROOT,
        DecisionAnchorLineageStageV1::Root,
    );
    state.on_tool_batch_finished(&[
        (
            "implementation-root",
            "codebase_memory_search_graph",
            &implementation_root,
        ),
        (
            "focused-root",
            "codebase_memory_search_graph",
            &focused_root,
        ),
    ]);

    let target = TargetAdmissionOutcome::Eligible(exact_target(TARGET_A));
    state.on_tool_dispatched(
        &source_call("forest-implementation", DecisionEvidenceKindV1::Implementation),
        1,
    );
    state.on_tool_finished_with_source_target(
        "forest-implementation",
        "codebase_memory_get_code_snippet",
        &output_with_evidence(
            ROOT,
            DecisionAnchorLineageStageV1::CarryForward,
            DecisionEvidenceKindV1::Implementation,
        ),
        Some(&target),
    );
    state.on_tool_dispatched(&call("forest-trace", "codebase_memory_trace_path"), 2);
    finish(
        &mut state,
        "forest-trace",
        "codebase_memory_trace_path",
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
    );
    state.on_tool_dispatched(
        &source_call("forest-caller", DecisionEvidenceKindV1::Caller),
        3,
    );
    finish_with_evidence(
        &mut state,
        "forest-caller",
        ROOT,
        DecisionEvidenceKindV1::Caller,
    );
    state.on_tool_dispatched(
        &source_call("forest-focused", DecisionEvidenceKindV1::FocusedTest),
        4,
    );
    assert_eq!(
        finish_with_evidence(
            &mut state,
            "forest-focused",
            OTHER_ROOT,
            DecisionEvidenceKindV1::FocusedTest,
        ),
        DecisionAnchorTransition::EnabledEvidenceComplete,
    );

    let mutation = call("forest-mutation", "write");
    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &mutation,
            5,
            Some(&mutation_targets(vec![target.clone()])),
        ),
        Some(ToolCallDenial::DecisionAnchorMutation),
    );
    let read = call("forest-exact-read", "read");
    state.on_tool_dispatched_with_targets(&read, 6, Some(&read_target(TARGET_A)));
    state.on_tool_finished("forest-exact-read", "read", &successful_read());
    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &mutation,
            7,
            Some(&mutation_targets(vec![target])),
        ),
        None,
    );
    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &call("forest-validation", "bash"),
            8,
            Some(&InvocationTargetAdmission::SourceNeutralProcess),
        ),
        None,
    );
    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &call("forest-submission", "submit_for_pr"),
            9,
            Some(&InvocationTargetAdmission::ControlPlane),
        ),
        None,
    );
}

#[test]
fn rejected_failed_cross_root_broad_and_irrelevant_calls_receive_no_evidence_credit() {
    const PRIVATE_SELECTOR: &str = "private::selector::must-not-leak";
    for status in [
        LineageAdmissionStatus::UnknownSelector,
        LineageAdmissionStatus::AmbiguousSelector,
        LineageAdmissionStatus::MalformedSelector,
        LineageAdmissionStatus::BroadSelector,
        LineageAdmissionStatus::UnsupportedTool,
    ] {
        let mut state = state_with_kinds(&[]);
        let mut rejected =
            source_call("rejected-source", DecisionEvidenceKindV1::Implementation);
        rejected.arguments["qualified_name"] = serde_json::json!(PRIVATE_SELECTOR);
        assert!(
            state
                .on_tool_dispatched_with_admission(
                    &rejected,
                    1,
                    Some(&LineageAdmissionOutcome::Ineligible(status)),
                )
                .is_some(),
            "{status:?} must be denied locally",
        );
        let guidance = state.take_model_guidance().join("\n");
        assert!(!guidance.contains(PRIVATE_SELECTOR));
        assert!(!guidance.contains(ROOT));
        assert_exact_read_cannot_release_incomplete_authority(&mut state, "rejected selector");
    }

    let mut failed = state_with_kinds(&[]);
    failed.on_tool_dispatched(
        &source_call("failed-source", DecisionEvidenceKindV1::Implementation),
        1,
    );
    failed.on_tool_finished(
        "failed-source",
        "codebase_memory_get_code_snippet",
        &failure_output("graph_lifecycle_denial"),
    );
    assert_exact_read_cannot_release_incomplete_authority(&mut failed, "failed source");

    let mut cross_root = state_with_kinds(&[]);
    cross_root.on_tool_dispatched(
        &source_call("cross-root", DecisionEvidenceKindV1::Implementation),
        1,
    );
    finish_with_evidence(
        &mut cross_root,
        "cross-root",
        OTHER_ROOT,
        DecisionEvidenceKindV1::Implementation,
    );
    assert_exact_read_cannot_release_incomplete_authority(&mut cross_root, "cross-root source");

    let mut broad = DecisionAnchorState::from_effects(&effects()).unwrap();
    for (turn, id) in [(0, "broad-once"), (1, "broad-repeated")] {
        broad.on_tool_dispatched(&call(id, "codebase_memory_get_architecture"), turn);
        broad.on_tool_finished(id, "codebase_memory_get_architecture", &plain_success());
    }
    assert_exact_read_cannot_release_incomplete_authority(
        &mut broad,
        "repeated broad and irrelevant success",
    );
}
