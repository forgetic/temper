fn creation_admission(existing: Vec<TargetAdmissionOutcome>) -> InvocationTargetAdmission {
    InvocationTargetAdmission::PatchCreation {
        existing,
        creations: vec![crate::MissingWorkspaceTarget::new(TARGET_B.to_string()).unwrap()],
    }
}

#[test]
fn patch_creation_requires_complete_graph_and_correction_inspection() {
    let admission = creation_admission(Vec::new());
    let patch = call("new-file", "apply_patch");
    let mut incomplete = DecisionAnchorState::from_effects(&effects()).unwrap();
    assert_eq!(
        incomplete.on_tool_dispatched_with_targets(&patch, 0, Some(&admission)),
        Some(ToolCallDenial::DecisionAnchorMutation),
    );
    let mut state = complete_state_with_correction();
    assert_eq!(
        state.on_tool_dispatched_with_targets(&patch, 5, Some(&admission)),
        Some(ToolCallDenial::DecisionAnchorMutation),
    );
    complete_correction_inspection(&mut state, 6);
    assert_eq!(
        state.on_tool_dispatched_with_targets(&patch, 7, Some(&admission)),
        None
    );
    assert_eq!(
        state.on_tool_dispatched_with_targets(&call("not-patch", "write"), 8, Some(&admission)),
        Some(ToolCallDenial::DecisionAnchorMutation),
        "absence proof is only authority for explicit patch creation",
    );
    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &call("creation-is-not-read", "write"),
            9,
            Some(&mutation_targets(vec![TargetAdmissionOutcome::Eligible(
                exact_target(TARGET_B)
            )])),
        ),
        Some(ToolCallDenial::DecisionAnchorMutation),
    );
}

#[test]
fn patch_creation_preserves_every_existing_siblings_exact_read_requirement() {
    let mut state = complete_state_with_correction();
    complete_correction_inspection(&mut state, 5);
    let existing = TargetAdmissionOutcome::Eligible(exact_target(TARGET_A));
    let mixed = creation_admission(vec![existing.clone()]);
    assert_eq!(
        state.on_tool_dispatched_with_targets(&call("mixed", "apply_patch"), 6, Some(&mixed)),
        Some(ToolCallDenial::DecisionAnchorMutation),
    );
    let read = call("exact-existing", "read");
    state.on_tool_dispatched_with_targets(&read, 7, Some(&read_target(TARGET_A)));
    state.on_tool_finished("exact-existing", "read", &failure_output("transport"));
    assert_eq!(
        state.on_tool_dispatched_with_targets(&call("failed-read", "apply_patch"), 8, Some(&mixed)),
        Some(ToolCallDenial::DecisionAnchorMutation),
    );
    state.on_tool_dispatched_with_targets(&read, 9, Some(&read_target(TARGET_A)));
    state.on_tool_finished("exact-existing", "read", &successful_read());
    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &call("ready-mixed", "apply_patch"),
            10,
            Some(&mixed)
        ),
        None
    );
    let invalid = creation_admission(vec![
        existing,
        TargetAdmissionOutcome::Ineligible(TargetAdmissionStatus::UnknownTarget),
    ]);
    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &call("invalid-sibling", "apply_patch"),
            11,
            Some(&invalid)
        ),
        Some(ToolCallDenial::DecisionAnchorMutation),
    );
}

#[test]
fn patch_creation_after_provider_fallback_does_not_authorize_existing_files() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    state.on_tool_dispatched(&call("unavailable", "codebase_memory_search_graph"), 0);
    assert_eq!(
        state.on_tool_finished(
            "unavailable",
            "codebase_memory_search_graph",
            &failure_output("transport")
        ),
        DecisionAnchorTransition::ProviderUnavailableFallback
    );
    let create = creation_admission(Vec::new());
    assert_eq!(
        state.on_tool_dispatched_with_targets(&call("new", "apply_patch"), 1, Some(&create)),
        None
    );
    let mixed = creation_admission(vec![TargetAdmissionOutcome::Eligible(exact_target(
        TARGET_A,
    ))]);
    assert_eq!(
        state.on_tool_dispatched_with_targets(&call("mixed", "apply_patch"), 2, Some(&mixed)),
        Some(ToolCallDenial::DecisionAnchorMutation)
    );
    state.on_tool_dispatched_with_targets(
        &call("existing", "read"),
        3,
        Some(&read_target(TARGET_A)),
    );
    state.on_tool_finished("existing", "read", &successful_read());
    assert_eq!(
        state.on_tool_dispatched_with_targets(&call("ready", "apply_patch"), 4, Some(&mixed)),
        None
    );
}

include!("decision_anchor_creation_batch.rs");
