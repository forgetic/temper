fn diagnostic_state(case: usize) -> DecisionAnchorState {
    match case {
        0 => DecisionAnchorState::from_effects(&effects()).unwrap(),
        1 => state_with_kinds(&[DecisionEvidenceKindV1::Implementation]),
        2 => state_with_kinds(&[
            DecisionEvidenceKindV1::Implementation,
            DecisionEvidenceKindV1::Caller,
            DecisionEvidenceKindV1::FocusedTest,
        ]),
        3 => companion_ready_state(),
        4 => complete_state_with_correction(),
        5 | 6 => {
            let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
            state.on_tool_dispatched(&call("unavailable", "codebase_memory_search_graph"), 0);
            state.on_tool_finished(
                "unavailable",
                "codebase_memory_search_graph",
                &failure_output("transport"),
            );
            if case == 6 {
                companion_read_result(&mut state, TARGET_A, 10, true, false);
            }
            state
        }
        _ => unreachable!(),
    }
}

fn invalid_diagnostic_admissions(status: TargetAdmissionStatus) -> Vec<InvocationTargetAdmission> {
    let invalid = TargetAdmissionOutcome::Ineligible(status);
    let valid = TargetAdmissionOutcome::Eligible(exact_target(TARGET_A));
    vec![
        InvocationTargetAdmission::Ineligible(status),
        mutation_targets(vec![invalid.clone()]),
        mutation_targets(vec![valid.clone(), invalid.clone()]),
        creation_admission(vec![valid, invalid]),
    ]
}

#[test]
fn mutation_diagnostics_preserve_denial_for_every_phase_and_nested_invalid_target() {
    for case in 0..7 {
        for (status, denial) in [
            (
                TargetAdmissionStatus::MalformedTarget,
                ToolCallDenial::MalformedMutationTarget,
            ),
            (
                TargetAdmissionStatus::CompetingTargets,
                ToolCallDenial::ConflictingMutationTargets,
            ),
            (
                TargetAdmissionStatus::UnknownTarget,
                ToolCallDenial::DecisionAnchorMutation,
            ),
            (
                TargetAdmissionStatus::AmbiguousTarget,
                ToolCallDenial::DecisionAnchorMutation,
            ),
            (
                TargetAdmissionStatus::OutsideWorkspace,
                ToolCallDenial::DecisionAnchorMutation,
            ),
            (
                TargetAdmissionStatus::UnsupportedTool,
                ToolCallDenial::DecisionAnchorMutation,
            ),
        ] {
            for admission in invalid_diagnostic_admissions(status) {
                let mut state = diagnostic_state(case);
                assert_eq!(
                    state.on_tool_dispatched_with_targets(
                        &call("invalid", "apply_patch"),
                        20,
                        Some(&admission)
                    ),
                    Some(denial.clone()),
                    "phase {case}, {status:?} must deny the entire mutation",
                );
                // A denied target cannot grant authority to an unrelated unread companion.
                assert_eq!(
                    companion_mutation(&mut state, 21, &[TARGET_B]),
                    Some(ToolCallDenial::DecisionAnchorMutation)
                );
            }
        }
    }
}

#[test]
fn mutation_diagnostics_preserve_valid_mutation_and_source_neutral_admission_matrix() {
    for case in 0..7 {
        let valid = mutation_targets(vec![TargetAdmissionOutcome::Eligible(exact_target(
            TARGET_A,
        ))]);
        for (admission, allowed) in [
            (valid, matches!(case, 3 | 6)),
            (
                creation_admission(Vec::new()),
                matches!(case, 2 | 3 | 5 | 6),
            ),
            (InvocationTargetAdmission::SourceNeutralProcess, true),
            (InvocationTargetAdmission::ControlPlane, true),
        ] {
            let denial = diagnostic_state(case).on_tool_dispatched_with_targets(
                &call("matrix", "apply_patch"),
                20,
                Some(&admission),
            );
            assert_eq!(denial.is_none(), allowed, "phase {case}");
        }
    }
    let read_only = effects()
        .into_iter()
        .filter(|(_, effect)| !effect.writes)
        .collect();
    let mut state = DecisionAnchorState::from_effects(&read_only).unwrap();
    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &call("no-mutation-policy", "apply_patch"),
            0,
            Some(&InvocationTargetAdmission::Ineligible(
                TargetAdmissionStatus::MalformedTarget
            )),
        ),
        None,
        "diagnostics must not add a new authorization gate"
    );
}

#[test]
fn mutation_diagnostics_leave_unknown_targets_on_the_successful_ordinary_read_path() {
    for creation in [false, true] {
        let mut state = companion_ready_state();
        let unknown = TargetAdmissionOutcome::Ineligible(TargetAdmissionStatus::UnknownTarget);
        let unread = if creation {
            creation_admission(vec![unknown])
        } else {
            mutation_targets(vec![unknown])
        };
        assert_eq!(
            state.on_tool_dispatched_with_targets(
                &call("unknown", "apply_patch"),
                11,
                Some(&unread),
            ),
            Some(ToolCallDenial::DecisionAnchorMutation)
        );
        let known = TargetAdmissionOutcome::Eligible(exact_target(TARGET_B));
        let resolved = if creation {
            creation_admission(vec![known])
        } else {
            mutation_targets(vec![known])
        };
        companion_read_result(&mut state, TARGET_B, 12, false, false);
        assert_eq!(
            state.on_tool_dispatched_with_targets(
                &call("failed-read", "apply_patch"),
                13,
                Some(&resolved),
            ),
            Some(ToolCallDenial::DecisionAnchorMutation)
        );
        companion_read_result(&mut state, TARGET_B, 14, true, false);
        assert_eq!(
            state.on_tool_dispatched_with_targets(
                &call("read-complete", "apply_patch"),
                15,
                Some(&resolved),
            ),
            None
        );
    }
}

#[test]
fn mutation_diagnostics_do_not_replace_graph_or_correction_denial_priority() {
    let malformed = InvocationTargetAdmission::Ineligible(TargetAdmissionStatus::MalformedTarget);
    let mut state = complete_state_with_correction();
    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &call("ordinary-read", "read"),
            5,
            Some(&read_target(TARGET_A)),
        ),
        Some(ToolCallDenial::DecisionAnchorCorrectionInspection)
    );
    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &call("closed-graph", "codebase_memory_search_graph"),
            6,
            Some(&malformed),
        ),
        completed_graph_denial()
    );
    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &call("malformed-patch", "apply_patch"),
            7,
            Some(&malformed),
        ),
        Some(ToolCallDenial::MalformedMutationTarget)
    );
    complete_correction_inspection(&mut state, 8);
    companion_read_result(&mut state, TARGET_A, 9, true, false);
    assert_eq!(companion_mutation(&mut state, 10, &[TARGET_A]), None);
}

#[test]
fn mutation_diagnostics_prioritize_malformed_arguments_independently_of_target_order() {
    for statuses in [
        [
            TargetAdmissionStatus::CompetingTargets,
            TargetAdmissionStatus::MalformedTarget,
        ],
        [
            TargetAdmissionStatus::MalformedTarget,
            TargetAdmissionStatus::CompetingTargets,
        ],
    ] {
        let targets = statuses.map(TargetAdmissionOutcome::Ineligible).to_vec();
        for admission in [
            mutation_targets(targets.clone()),
            creation_admission(targets),
        ] {
            assert_eq!(
                companion_ready_state().on_tool_dispatched_with_targets(
                    &call("mixed-errors", "apply_patch"),
                    20,
                    Some(&admission),
                ),
                Some(ToolCallDenial::MalformedMutationTarget)
            );
        }
    }
}
