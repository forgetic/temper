// Companion edits exercise the same dispatch/settlement path as primary edits.

fn companion_ready_state() -> DecisionAnchorState {
    let mut state = state_with_kinds(&[
        DecisionEvidenceKindV1::Implementation,
        DecisionEvidenceKindV1::Caller,
        DecisionEvidenceKindV1::FocusedTest,
    ]);
    companion_read_result(&mut state, TARGET_A, 10, true, false);
    state
}

fn companion_read_result(
    state: &mut DecisionAnchorState,
    target: &str,
    turn: usize,
    succeeded: bool,
    is_error: bool,
) {
    let id = format!("companion-read-{turn}");
    assert_eq!(
        state.on_tool_dispatched_with_targets(&call(&id, "read"), turn, Some(&read_target(target))),
        None
    );
    let mut output = successful_read();
    output.is_error = is_error;
    state.on_tool_batch_finished_with_targets(&[(&id, "read", &output, None, succeeded)]);
}

fn companion_mutation(
    state: &mut DecisionAnchorState,
    turn: usize,
    targets: &[&str],
) -> Option<ToolCallDenial> {
    let targets = targets
        .iter()
        .map(|target| TargetAdmissionOutcome::Eligible(exact_target(target)))
        .collect();
    state.on_tool_dispatched_with_targets(
        &call(&format!("companion-mutation-{turn}"), "write"),
        turn,
        Some(&mutation_targets(targets)),
    )
}

#[test]
fn companion_source_test_and_documentation_reads_authorize_one_cohesive_mutation() {
    let mut state = companion_ready_state();
    let targets = [
        TARGET_B,
        "00000000-0000-4000-8000-000000000013",
        "00000000-0000-4000-8000-000000000014",
    ];
    for (offset, target) in targets.iter().enumerate() {
        assert_eq!(
            companion_mutation(&mut state, 11 + offset * 2, &[target]),
            Some(ToolCallDenial::DecisionAnchorMutation)
        );
        companion_read_result(&mut state, target, 12 + offset * 2, true, false);
    }
    assert_eq!(
        companion_mutation(
            &mut state,
            20,
            &[TARGET_A, targets[0], targets[1], targets[2]]
        ),
        None
    );
    assert_eq!(
        state.on_tool_dispatched(
            &call("companion-closed-graph", "codebase_memory_search_graph"),
            21
        ),
        completed_graph_denial(),
        "companion reads do not reopen graph exploration"
    );
}

#[test]
fn companion_reads_before_primary_or_failed_reads_cannot_authorize_edits() {
    let mut state = state_with_kinds(&[
        DecisionEvidenceKindV1::Implementation,
        DecisionEvidenceKindV1::Caller,
        DecisionEvidenceKindV1::FocusedTest,
    ]);
    companion_read_result(&mut state, TARGET_B, 10, true, false);
    companion_read_result(&mut state, TARGET_A, 11, true, false);
    assert_eq!(
        companion_mutation(&mut state, 12, &[TARGET_B]),
        Some(ToolCallDenial::DecisionAnchorMutation),
        "a companion read before the primary read cannot gain retroactive authority"
    );
    for (turn, succeeded, is_error) in [(13, false, false), (15, true, true)] {
        companion_read_result(&mut state, TARGET_B, turn, succeeded, is_error);
        assert_eq!(
            companion_mutation(&mut state, turn + 1, &[TARGET_B]),
            Some(ToolCallDenial::DecisionAnchorMutation)
        );
    }
    companion_read_result(&mut state, TARGET_B, 17, true, false);
    assert_eq!(
        companion_mutation(&mut state, 18, &[TARGET_A, TARGET_B]),
        None
    );
    assert_eq!(
        companion_mutation(
            &mut state,
            19,
            &[TARGET_A, TARGET_B, "00000000-0000-4000-8000-000000000015"]
        ),
        Some(ToolCallDenial::DecisionAnchorMutation),
        "an unread target blocks the whole patch"
    );
}

#[test]
fn companion_reads_do_not_complete_incomplete_graph_evidence() {
    for kinds in [
        vec![],
        vec![DecisionEvidenceKindV1::Implementation],
        vec![
            DecisionEvidenceKindV1::Implementation,
            DecisionEvidenceKindV1::Caller,
        ],
        vec![
            DecisionEvidenceKindV1::Implementation,
            DecisionEvidenceKindV1::FocusedTest,
        ],
    ] {
        let mut state = state_with_kinds(&kinds);
        companion_read_result(&mut state, TARGET_A, 10, true, false);
        companion_read_result(&mut state, TARGET_B, 11, true, false);
        assert_eq!(
            companion_mutation(&mut state, 12, &[TARGET_A, TARGET_B]),
            Some(ToolCallDenial::DecisionAnchorMutation)
        );
    }
}

#[test]
fn companion_authority_requires_an_ordinary_read_of_an_eligible_host_target() {
    let mut state = companion_ready_state();
    let other_tool = call("companion-grep", "grep");
    state.on_tool_dispatched_with_targets(&other_tool, 11, Some(&read_target(TARGET_B)));
    state.on_tool_finished(&other_tool.id, "grep", &successful_read());
    for (offset, status) in [
        TargetAdmissionStatus::OutsideWorkspace,
        TargetAdmissionStatus::UnknownTarget,
        TargetAdmissionStatus::MalformedTarget,
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("companion-ineligible-{offset}");
        let admission = InvocationTargetAdmission::Read(TargetAdmissionOutcome::Ineligible(status));
        state.on_tool_dispatched_with_targets(&call(&id, "read"), 12 + offset, Some(&admission));
        state.on_tool_finished(&id, "read", &successful_read());
    }
    assert_eq!(
        companion_mutation(&mut state, 20, &[TARGET_B]),
        Some(ToolCallDenial::DecisionAnchorMutation)
    );
    companion_read_result(&mut state, TARGET_B, 21, true, false);
    let mixed = mutation_targets(vec![
        TargetAdmissionOutcome::Eligible(exact_target(TARGET_B)),
        TargetAdmissionOutcome::Ineligible(TargetAdmissionStatus::OutsideWorkspace),
    ]);
    assert_eq!(
        state.on_tool_dispatched_with_targets(&call("companion-mixed", "write"), 22, Some(&mixed)),
        Some(ToolCallDenial::DecisionAnchorMutation)
    );
}
