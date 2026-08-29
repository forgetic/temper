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
    assert!(!early_guidance.contains("required next stage=[search_graph/graph_query/focused_test"));

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
        "required next stage=[search_graph/graph_query/focused_test/selector=task_semantic_query]"
    ));
}

#[test]
fn semantic_test_stage_rejects_initial_result_and_traversal_detours_before_exact_source() {
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

    let initial_test = source_call("initial-search-test", DecisionEvidenceKindV1::FocusedTest);
    assert_eq!(
        state.on_tool_dispatched_with_admission(
            &initial_test,
            4,
            Some(&LineageAdmissionOutcome::Eligible(source_admission(
                DecisionEvidenceKindV1::FocusedTest,
            ))),
        ),
        recovery_graph_denial([GraphRecoveryEvidenceKindV1::FocusedTest], 4),
    );
    let initial_guidance = one_guidance(&mut state);
    assert!(initial_guidance.contains(
        "required next stage=[search_graph/graph_query/focused_test/selector=task_semantic_query]"
    ));

    let mut traversal = call("focused-traversal-detour", "codebase_memory_trace_path");
    traversal.arguments = serde_json::json!({
        "function_name": "provider-returned-caller",
        "mode": "calls",
        "direction": "inbound",
        "include_tests": true,
    });
    assert_eq!(
        state.on_tool_dispatched_with_admission(
            &traversal,
            5,
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
    let traversal_guidance = one_guidance(&mut state);
    assert!(traversal_guidance.contains(
        "required next stage=[search_graph/graph_query/focused_test/selector=task_semantic_query]"
    ));

    let search = semantic_search_call("semantic-test-search");
    assert_eq!(
        state.on_tool_dispatched_with_admission(
            &search,
            6,
            Some(&LineageAdmissionOutcome::Eligible(
                semantic_search_admission(),
            )),
        ),
        None,
    );
    state.on_tool_finished(
        "semantic-test-search",
        "codebase_memory_search_graph",
        &output_with_focused_test_discovery(
            "codebase_memory_search_graph",
            ROOT,
            DecisionAnchorLineageStageV1::CarryForward,
            FocusedTestDiscoveryOutcomeV1::EligibleSelectorReturned,
        ),
    );
    let search_guidance = one_guidance(&mut state);
    assert!(search_guidance.contains("accepted evidence=[focused_test_route]"));
    assert!(search_guidance.contains(
        "required next stage=[get_code_snippet/qualified_name/focused_test/selector=focused_test_result]"
    ));

    let mut exact_test = source_call("exact-test", DecisionEvidenceKindV1::FocusedTest);
    exact_test.arguments["qualified_name"] = serde_json::json!("semantic-search-returned-test");
    assert_eq!(
        state.on_tool_dispatched_with_admission(
            &exact_test,
            7,
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

#[test]
fn acceptance_002_through_005_missing_stages_have_one_deterministic_continuation() {
    for case in ["003", "005"] {
        let mut state = state_through_caller_trace();
        let early_test = source_call(case, DecisionEvidenceKindV1::FocusedTest);
        assert_eq!(
            state.on_tool_dispatched_with_admission(
                &early_test,
                3,
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
            "acceptance {case} must not retain focused-test evidence before caller source",
        );
        let guidance = one_guidance(&mut state);
        assert!(guidance.contains(
            "required next stage=[get_code_snippet/qualified_name/caller/selector=caller_traversal_result]"
        ));
    }

    let mut case_004 = DecisionAnchorState::from_effects(&effects()).unwrap();
    case_004.on_tool_dispatched(&call("root-004", "codebase_memory_search_graph"), 0);
    case_004.on_tool_finished(
        "root-004",
        "codebase_memory_search_graph",
        &output(
            "codebase_memory_search_graph",
            ROOT,
            DecisionAnchorLineageStageV1::Root,
        ),
    );
    case_004.take_model_guidance();
    case_004.on_tool_dispatched(
        &source_call(
            "implementation-004",
            DecisionEvidenceKindV1::Implementation,
        ),
        1,
    );
    finish_with_evidence(
        &mut case_004,
        "implementation-004",
        ROOT,
        DecisionEvidenceKindV1::Implementation,
    );
    let guidance_004 = one_guidance(&mut case_004);
    assert!(guidance_004.contains(
        "required next stage=[trace_path/function_name/trace/selector=implementation_evidence_result/relationship=calls/direction=inbound]"
    ));

    let mut case_002 = state_through_caller_trace();
    let caller = source_call("caller-002", DecisionEvidenceKindV1::Caller);
    case_002.on_tool_dispatched_with_admission(
        &caller,
        3,
        Some(&LineageAdmissionOutcome::Eligible(source_admission(
            DecisionEvidenceKindV1::Caller,
        ))),
    );
    finish_with_evidence(
        &mut case_002,
        "caller-002",
        ROOT,
        DecisionEvidenceKindV1::Caller,
    );
    let guidance_002 = one_guidance(&mut case_002);
    assert!(guidance_002.contains(
        "required next stage=[search_graph/graph_query/focused_test/selector=task_semantic_query]"
    ));
}

#[test]
fn incomplete_traversal_is_denied_without_spending_recovery_and_then_recovers() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    state.on_tool_dispatched(&call("root", "codebase_memory_search_graph"), 0);
    state.on_tool_finished(
        "root",
        "codebase_memory_search_graph",
        &output(
            "codebase_memory_search_graph",
            ROOT,
            DecisionAnchorLineageStageV1::Root,
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

    let mut unknown = call("unknown-function-name", "codebase_memory_trace_path");
    unknown.arguments = serde_json::json!({
        "function_name": "schema-valid-but-never-returned",
        "direction": "inbound",
    });
    assert_eq!(
        state.on_tool_batch_dispatched_with_closed_inputs(
            &[unknown],
            2,
            &[None],
            &[None],
            &[None],
        ),
        [recovery_graph_denial(
            [
                GraphRecoveryEvidenceKindV1::Trace,
                GraphRecoveryEvidenceKindV1::Caller,
                GraphRecoveryEvidenceKindV1::FocusedTest,
            ],
            4,
        )],
    );
    let denied = one_guidance(&mut state);
    assert!(denied.contains("result=non_progress"));
    assert!(denied.contains(
        "required next stage=[trace_path/function_name/trace/selector=implementation_evidence_result/relationship=calls/direction=inbound]"
    ));

    let scrubbed = call("missing-function-name", crate::REJECTED_TOOL_NAME);
    assert_eq!(
        state.on_tool_batch_dispatched_with_closed_inputs(
            &[scrubbed],
            2,
            &[None],
            &[None],
            &[Some(GraphCorrelationToolV1::TracePath)],
        ),
        [recovery_graph_denial(
            [
                GraphRecoveryEvidenceKindV1::Trace,
                GraphRecoveryEvidenceKindV1::Caller,
                GraphRecoveryEvidenceKindV1::FocusedTest,
            ],
            4,
        )],
    );
    let denied = one_guidance(&mut state);
    assert!(denied.contains("result=non_progress"));
    assert!(denied.contains(
        "required next stage=[trace_path/function_name/trace/selector=implementation_evidence_result/relationship=calls/direction=inbound]"
    ));

    let mut recovered = call("complete-trace", "codebase_memory_trace_path");
    recovered.arguments = serde_json::json!({
        "function_name": "provider-returned-implementation",
        "direction": "inbound",
    });
    let admission = EligibleLineageAdmission::implementation_caller_traversal(
        ROOT.to_string(),
        DecisionAnchorTargetKindV1::FunctionName,
    )
    .unwrap();
    assert_eq!(
        state.on_tool_dispatched_with_admission(
            &recovered,
            3,
            Some(&LineageAdmissionOutcome::Eligible(admission)),
        ),
        None,
    );
    state.on_tool_finished(
        "complete-trace",
        "codebase_memory_trace_path",
        &output_with_caller_discovery(
            ROOT,
            DecisionAnchorLineageStageV1::CarryForward,
            CallerDiscoveryOutcomeV1::EligibleSelectorReturned,
        ),
    );
    let recovered = one_guidance(&mut state);
    assert!(recovered.contains("accepted evidence=[trace]"));
    assert!(recovered.contains("get_code_snippet/qualified_name/caller"));
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
        &source_call(
            "implementation",
            DecisionEvidenceKindV1::Implementation,
        ),
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

fn semantic_search_call(id: &str) -> ToolCall {
    let mut search = call(id, "codebase_memory_search_graph");
    search.arguments = serde_json::json!({"query": "behavioral regression intent"});
    search
}

fn semantic_search_admission() -> EligibleLineageAdmission {
    EligibleLineageAdmission::focused_test_semantic_fallback(
        ROOT.to_string(),
        DecisionAnchorTargetKindV1::GraphQuery,
    )
    .unwrap()
}

fn one_guidance(state: &mut DecisionAnchorState) -> String {
    let guidance = state.take_model_guidance();
    assert_eq!(guidance.len(), 1);
    guidance.into_iter().next().unwrap()
}
