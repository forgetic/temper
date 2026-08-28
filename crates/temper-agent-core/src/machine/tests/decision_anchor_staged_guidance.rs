// Exact next-stage guidance and local detour rejection regressions.

#[test]
fn missing_caller_rejects_caller_shaped_and_focused_sources_with_one_exact_next_stage() {
    let mut state = state_through_caller_trace();

    let mut wrong_caller = source_call("wrong-caller", DecisionEvidenceKindV1::Caller);
    wrong_caller.arguments["qualified_name"] = serde_json::json!("caller-shaped-not-returned");
    assert_eq!(
        state.on_tool_dispatched_with_admission(
            &wrong_caller,
            3,
            Some(&LineageAdmissionOutcome::Ineligible(
                LineageAdmissionStatus::IncapableSelection,
            )),
        ),
        recovery_graph_denial(
            [
                GraphRecoveryEvidenceKindV1::Caller,
                GraphRecoveryEvidenceKindV1::FocusedTest,
            ],
            4,
        ),
    );
    let wrong_guidance = one_guidance(&mut state);
    assert!(wrong_guidance.contains("result=non_progress"));
    assert!(wrong_guidance.contains("accepted evidence=[]"));
    assert!(wrong_guidance.contains(
        "required next stage=[get_code_snippet/qualified_name/caller/selector=caller_traversal_result]"
    ));
    assert!(wrong_guidance.contains("do not repeat that selector value"));

    let early_test = source_call("early-test", DecisionEvidenceKindV1::FocusedTest);
    assert_eq!(
        state.on_tool_dispatched_with_admission(
            &early_test,
            4,
            Some(&LineageAdmissionOutcome::Eligible(source_admission(
                DecisionEvidenceKindV1::FocusedTest,
            ))),
        ),
        recovery_graph_denial(
            [
                GraphRecoveryEvidenceKindV1::Caller,
                GraphRecoveryEvidenceKindV1::FocusedTest,
            ],
            4,
        ),
    );
    let early_guidance = one_guidance(&mut state);
    assert!(early_guidance.contains("required next stage=[get_code_snippet/qualified_name/caller"));
    assert!(!early_guidance
        .contains("required next stage=[get_code_snippet/qualified_name/focused_test"));

    let mut caller = source_call("exact-caller", DecisionEvidenceKindV1::Caller);
    caller.arguments["qualified_name"] = serde_json::json!("provider-returned-caller");
    assert_eq!(
        state.on_tool_dispatched_with_admission(
            &caller,
            5,
            Some(&LineageAdmissionOutcome::Eligible(source_admission(
                DecisionEvidenceKindV1::Caller,
            ))),
        ),
        None,
    );
    finish_with_evidence(
        &mut state,
        "exact-caller",
        ROOT,
        DecisionEvidenceKindV1::Caller,
    );
    let caller_guidance = one_guidance(&mut state);
    assert!(caller_guidance.contains("accepted evidence=[caller]"));
    assert!(caller_guidance.contains(
        "required next stage=[get_code_snippet/qualified_name/focused_test/selector=focused_test_result]"
    ));
}

#[test]
fn semantic_test_selector_rejects_repeated_traversal_detours_before_exact_source() {
    let mut state = state_through_caller_trace();
    let caller = source_call("caller", DecisionEvidenceKindV1::Caller);
    state.on_tool_dispatched_with_admission(
        &caller,
        3,
        Some(&LineageAdmissionOutcome::Eligible(source_admission(
            DecisionEvidenceKindV1::Caller,
        ))),
    );
    finish_with_evidence(
        &mut state,
        "caller",
        ROOT,
        DecisionEvidenceKindV1::Caller,
    );
    state.take_model_guidance();

    for (turn, id) in [(4, "focused-detour"), (5, "focused-detour-repeat")] {
        let mut traversal = call(id, "codebase_memory_trace_path");
        traversal.arguments = serde_json::json!({
            "function_name": "provider-returned-caller",
            "mode": "calls",
            "direction": "inbound",
            "include_tests": true,
        });
        assert_eq!(
            state.on_tool_dispatched_with_admission(
                &traversal,
                turn,
                Some(&LineageAdmissionOutcome::Eligible(
                    EligibleLineageAdmission::focused_test_traversal(
                        ROOT.to_string(),
                        DecisionAnchorTargetKindV1::FunctionName,
                    )
                    .unwrap(),
                )),
            ),
            recovery_graph_denial([GraphRecoveryEvidenceKindV1::FocusedTest], 4),
        );
        let guidance = one_guidance(&mut state);
        assert!(guidance.contains("result=non_progress"));
        assert!(guidance.contains(
            "required next stage=[get_code_snippet/qualified_name/focused_test/selector=focused_test_result]"
        ));
        assert!(!guidance.contains("required next stage=[trace_path/function_name/focused_test"));
    }

    let exact_test = source_call("exact-test", DecisionEvidenceKindV1::FocusedTest);
    assert_eq!(
        state.on_tool_dispatched_with_admission(
            &exact_test,
            6,
            Some(&LineageAdmissionOutcome::Eligible(source_admission(
                DecisionEvidenceKindV1::FocusedTest,
            ))),
        ),
        None,
    );
    assert_eq!(
        finish_with_evidence(
            &mut state,
            "exact-test",
            ROOT,
            DecisionEvidenceKindV1::FocusedTest,
        ),
        DecisionAnchorTransition::Converged,
    );
}

fn state_through_caller_trace() -> DecisionAnchorState {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    state.on_tool_dispatched(&call("root", "codebase_memory_search_graph"), 0);
    state.on_tool_finished(
        "root",
        "codebase_memory_search_graph",
        &output_with_focused_test_discovery(
            "codebase_memory_search_graph",
            ROOT,
            DecisionAnchorLineageStageV1::Root,
            FocusedTestDiscoveryOutcomeV1::EligibleSelectorReturned,
        ),
    );
    state.take_model_guidance();
    state.on_tool_dispatched(
        &source_call("implementation", DecisionEvidenceKindV1::Implementation),
        1,
    );
    finish_with_evidence(
        &mut state,
        "implementation",
        ROOT,
        DecisionEvidenceKindV1::Implementation,
    );
    state.take_model_guidance();
    state.on_tool_dispatched(&call("caller-trace", "codebase_memory_trace_path"), 2);
    state.on_tool_finished(
        "caller-trace",
        "codebase_memory_trace_path",
        &output_with_caller_discovery(
            ROOT,
            DecisionAnchorLineageStageV1::CarryForward,
            CallerDiscoveryOutcomeV1::EligibleSelectorReturned,
        ),
    );
    state.take_model_guidance();
    state
}

fn source_admission(kind: DecisionEvidenceKindV1) -> EligibleLineageAdmission {
    EligibleLineageAdmission::new(
        ROOT.to_string(),
        DecisionAnchorTargetKindV1::QualifiedName,
        GraphCorrelationToolV1::GetCodeSnippet,
        Some(kind),
    )
    .unwrap()
}

fn one_guidance(state: &mut DecisionAnchorState) -> String {
    let guidance = state.take_model_guidance();
    assert_eq!(guidance.len(), 1);
    guidance.into_iter().next().unwrap()
}
