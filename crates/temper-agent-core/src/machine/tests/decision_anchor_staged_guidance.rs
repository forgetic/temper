// Exact next-stage guidance and bounded local rejection regressions.

fn state_through_caller_trace() -> DecisionAnchorState {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    state.on_tool_dispatched(&call("root", "codebase_memory_search_graph"), 0);
    finish(
        &mut state,
        "root",
        "codebase_memory_search_graph",
        ROOT,
        DecisionAnchorLineageStageV1::Root,
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
    state.on_tool_dispatched(&call("trace", "codebase_memory_trace_path"), 2);
    state.on_tool_finished(
        "trace",
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

fn staged_source_admission(kind: DecisionEvidenceKindV1) -> LineageAdmissionOutcome {
    LineageAdmissionOutcome::Eligible(
        EligibleLineageAdmission::new(
            ROOT.to_string(),
            DecisionAnchorTargetKindV1::QualifiedName,
            GraphCorrelationToolV1::GetCodeSnippet,
            Some(kind),
        )
        .unwrap(),
    )
}

#[test]
fn missing_caller_exposes_one_provider_derived_continuation() {
    let mut state = state_through_caller_trace();
    let mut wrong = source_call("wrong", DecisionEvidenceKindV1::Caller);
    wrong.arguments["qualified_name"] = serde_json::json!("not-returned");
    assert!(
        state
            .on_tool_dispatched_with_admission(
                &wrong,
                3,
                Some(&LineageAdmissionOutcome::Ineligible(
                    LineageAdmissionStatus::UnknownSelector,
                )),
            )
            .is_some()
    );
    let guidance = state.take_model_guidance();
    assert_eq!(guidance.len(), 1);
    assert!(guidance[0].contains("remaining allowance=3"));
    assert!(guidance[0].contains(
        "required next stage=[get_code_snippet/qualified_name/caller/selector=caller_traversal_result]"
    ));
    assert!(guidance[0].contains("do not repeat that selector value"));
    assert!(!guidance[0].contains("not-returned"));

    let caller = source_call("caller", DecisionEvidenceKindV1::Caller);
    assert_eq!(
        state.on_tool_dispatched_with_admission(
            &caller,
            4,
            Some(&staged_source_admission(DecisionEvidenceKindV1::Caller)),
        ),
        None,
    );
    assert_eq!(
        finish_with_evidence(&mut state, "caller", ROOT, DecisionEvidenceKindV1::Caller),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );
    let next = state.recovery_details().expect("focused source continuation");
    assert_eq!(
        next.compatible_actions,
        [GraphRecoveryActionV1::for_evidence(
            GraphRecoveryEvidenceKindV1::FocusedTest,
        )],
    );
}

#[test]
fn repeated_rejected_selector_tuples_exhaust_without_reopening_exploration() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    state.on_tool_dispatched(&call("root", "codebase_memory_search_graph"), 0);
    finish(
        &mut state,
        "root",
        "codebase_memory_search_graph",
        ROOT,
        DecisionAnchorLineageStageV1::Root,
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
        state.take_model_guidance();
    }

    let mut attempted = source_call("rejected", DecisionEvidenceKindV1::Implementation);
    attempted.arguments["qualified_name"] = serde_json::json!("malformed-or-repeated");
    for index in 0..temper_protocol_activity::MAX_GRAPH_RECOVERY_ALLOWANCE_V1 {
        attempted.id = format!("rejected-{index}");
        assert!(
            state
                .on_tool_dispatched_with_admission(
                    &attempted,
                    usize::from(index) + 3,
                    Some(&LineageAdmissionOutcome::Ineligible(
                        LineageAdmissionStatus::MalformedSelector,
                    )),
                )
                .is_some()
        );
        let guidance = state.take_model_guidance();
        assert_eq!(guidance.len(), 1);
        assert!(!guidance[0].contains("malformed-or-repeated"));
    }
    assert_eq!(
        state.on_tool_dispatched(&source_call("after-exhaustion", DecisionEvidenceKindV1::Implementation), 8),
        exhausted_graph_denial(all_missing()),
    );
    assert!(state.blocks_mutation("write"));
}
