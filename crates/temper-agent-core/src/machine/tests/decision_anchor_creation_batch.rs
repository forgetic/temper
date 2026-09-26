fn creation_correction_admission() -> LineageAdmissionOutcome {
    LineageAdmissionOutcome::Eligible(
        EligibleLineageAdmission::implementation_authority_correction(ROOT.to_string(), false)
            .unwrap(),
    )
}

fn creation_with_existing(target: &str) -> InvocationTargetAdmission {
    creation_admission(vec![TargetAdmissionOutcome::Eligible(exact_target(target))])
}

#[test]
fn patch_creation_and_correction_have_one_authority_outcome_in_both_batch_orders() {
    for correction_first in [true, false] {
        let mut state = complete_state_with_correction();
        complete_correction_inspection(&mut state, 5);
        let mut calls = vec![
            source_call("correction", DecisionEvidenceKindV1::Implementation),
            call("create", "apply_patch"),
        ];
        let mut admissions = vec![Some(creation_correction_admission()), None];
        let mut targets = vec![None, Some(creation_admission(Vec::new()))];
        let mut expected = vec![completed_graph_denial(), None];
        if !correction_first {
            calls.reverse();
            admissions.reverse();
            targets.reverse();
            expected.reverse();
        }
        assert_eq!(
            state.on_tool_batch_dispatched_with_admissions_and_targets(
                &calls,
                6,
                &admissions,
                &targets,
            ),
            expected,
            "creation closes correction before any sibling is dispatched",
        );
        state.on_tool_finished("create", "apply_patch", &successful_read());
        assert_eq!(state.active_recovery_action(), None);
        for target in [TARGET_A, TARGET_B] {
            assert_eq!(
                state.on_tool_dispatched_with_targets(
                    &call("unread-existing", "apply_patch"),
                    7,
                    Some(&creation_with_existing(target)),
                ),
                Some(ToolCallDenial::DecisionAnchorMutation),
                "creation must not authorize either existing implementation",
            );
        }
        state.on_tool_dispatched_with_targets(
            &call("retained-primary", "read"),
            8,
            Some(&read_target(TARGET_A)),
        );
        state.on_tool_finished("retained-primary", "read", &successful_read());
        assert_eq!(
            state.on_tool_dispatched_with_targets(
                &call("retained-mixed", "apply_patch"),
                9,
                Some(&creation_with_existing(TARGET_A)),
            ),
            None,
        );
    }
}

#[test]
fn patch_creation_with_corrected_existing_target_still_requires_its_fresh_read() {
    let mut state = complete_state_with_correction();
    complete_correction_inspection(&mut state, 5);
    state.on_tool_dispatched_with_targets(
        &call("before-correction", "read"),
        6,
        Some(&read_target(TARGET_B)),
    );
    state.on_tool_finished("before-correction", "read", &successful_read());
    let correction = source_call("correction", DecisionEvidenceKindV1::Implementation);
    assert_eq!(
        state.on_tool_dispatched_with_admission(
            &correction,
            7,
            Some(&creation_correction_admission()),
        ),
        None,
    );
    let mut output = output_with_evidence(
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
        DecisionEvidenceKindV1::Implementation,
    );
    output.details.as_mut().unwrap()[SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY]["implementation_authority_corrected"] =
        serde_json::json!(true);
    state.on_tool_finished_with_source_target(
        "correction",
        "codebase_memory_get_code_snippet",
        &output,
        Some(&TargetAdmissionOutcome::Eligible(exact_target(TARGET_B))),
    );
    state.on_tool_dispatched_with_targets(
        &call("stale-primary", "read"),
        8,
        Some(&read_target(TARGET_A)),
    );
    state.on_tool_finished("stale-primary", "read", &successful_read());
    for target in [TARGET_A, TARGET_B] {
        assert_eq!(
            state.on_tool_dispatched_with_targets(
                &call("before-corrected-read", "apply_patch"),
                9,
                Some(&creation_with_existing(target)),
            ),
            Some(ToolCallDenial::DecisionAnchorMutation),
        );
    }
    state.on_tool_dispatched_with_targets(
        &call("corrected-primary", "read"),
        10,
        Some(&read_target(TARGET_B)),
    );
    state.on_tool_finished("corrected-primary", "read", &successful_read());
    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &call("corrected-mixed", "apply_patch"),
            11,
            Some(&creation_with_existing(TARGET_B)),
        ),
        None,
    );
}
