use super::*;

#[test]
fn reported_zero_callers_still_requires_independent_test_and_exact_read_before_mutation() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    enter_two_root_recovery(&mut state);
    let route_target = TargetAdmissionOutcome::Eligible(workspace_target(ROUTE_TARGET));
    assert_eq!(
        recover_source(
            &mut state,
            "implementation",
            ROOT,
            DecisionEvidenceKindV1::Implementation,
            3,
            Some(&route_target)
        ),
        DecisionAnchorTransition::GapRecoveryNeeded
    );
    assert_eq!(
        recover_trace(
            &mut state,
            "empty-trace",
            ROOT,
            4,
            CallerDiscoveryOutcomeV1::NoProductionCallersReported
        ),
        DecisionAnchorTransition::GapRecoveryNeeded
    );
    assert_exposed_action(&state, OTHER_ROOT, GraphRecoveryEvidenceKindV1::FocusedTest);
    let mutation = InvocationTargetAdmission::Mutation(vec![route_target.clone()]);
    assert_eq!(
        state.on_tool_dispatched_with_targets(&call("before-test", "write"), 5, Some(&mutation)),
        Some(ToolCallDenial::DecisionAnchorMutation)
    );
    assert_eq!(
        recover_source(
            &mut state,
            "focused-test",
            OTHER_ROOT,
            DecisionEvidenceKindV1::FocusedTest,
            6,
            None
        ),
        DecisionAnchorTransition::EnabledEvidenceComplete
    );
    assert_post_read_authority(&mut state, route_target);
}

#[test]
fn sibling_zero_caller_report_cannot_complete_the_active_implementation() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    enter_two_root_recovery(&mut state);
    recover_source(
        &mut state,
        "implementation",
        ROOT,
        DecisionEvidenceKindV1::Implementation,
        3,
        None,
    );
    let trace = call("wrong-root-trace", "codebase_memory_trace_path");
    assert!(
        state
            .on_tool_dispatched_with_admission(&trace, 4, Some(&trace_admission(OTHER_ROOT)))
            .is_some()
    );
    assert_exposed_action(&state, ROOT, GraphRecoveryEvidenceKindV1::Trace);
}
