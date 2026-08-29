// Decision-anchor recovery and malformed-result regressions.

use super::*;
use crate::{EligibleLineageAdmission, LineageAdmissionOutcome};
use temper_protocol_activity::MAX_GRAPH_RECOVERY_ALLOWANCE_V1;

pub(super) fn conventional_fallback_graph_denial() -> Option<ToolCallDenial> {
    Some(ToolCallDenial::GraphExplorationClosed(Some(
        GraphExplorationClosedV1::conventional_fallback(),
    )))
}

mod batch {
    include!("decision_anchor_recovery_batch.rs");
}

fn enter_budget_recovery(state: &mut DecisionAnchorState, first_turn: usize) {
    for (offset, id, expected) in [
        (0, "broad-one", DecisionAnchorTransition::Unchanged),
        (1, "broad-two", DecisionAnchorTransition::GapRecoveryNeeded),
    ] {
        let turn = first_turn + offset;
        assert_eq!(
            state.on_tool_dispatched(&call(id, "codebase_memory_get_architecture"), turn),
            None,
        );
        assert_eq!(
            state.on_tool_finished(id, "codebase_memory_get_architecture", &plain_success()),
            expected,
        );
    }
}

fn install_consumable_root(state: &mut DecisionAnchorState) {
    state.on_tool_dispatched(&call("root", "codebase_memory_search_graph"), 0);
    finish(
        state,
        "root",
        "codebase_memory_search_graph",
        ROOT,
        DecisionAnchorLineageStageV1::Root,
    );
}

#[test]
fn unconsumable_roots_have_two_recovery_attempts_then_stay_blocked() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    for (turn, id, expected) in [
        (0, "root", DecisionAnchorTransition::RecoveryNeeded),
        (1, "recovery-one", DecisionAnchorTransition::RecoveryNeeded),
        (
            2,
            "recovery-two",
            DecisionAnchorTransition::RecoveryExhausted,
        ),
    ] {
        state.on_tool_dispatched(&call(id, "codebase_memory_search_graph"), turn);
        assert_eq!(
            finish_with_kinds(
                &mut state,
                id,
                "codebase_memory_search_graph",
                ROOT,
                DecisionAnchorLineageStageV1::Root,
                &[],
            ),
            expected
        );
        assert!(state.blocks_mutation("write"));
    }
}

#[test]
fn failed_or_malformed_graph_results_create_no_anchor_or_mutation_block() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    state.on_tool_dispatched(&call("failed", "codebase_memory_search_graph"), 0);
    let failed = ToolOutput {
        content: Vec::new(),
        details: None,
        is_error: true,
    };
    assert_eq!(
        state.on_tool_finished("failed", "codebase_memory_search_graph", &failed),
        DecisionAnchorTransition::Unchanged
    );
    assert!(!state.blocks_mutation("write"));

    state.on_tool_dispatched(&call("malformed", "codebase_memory_search_graph"), 1);
    let malformed = ToolOutput {
        content: Vec::new(),
        details: Some(serde_json::json!({
            SAFE_GRAPH_CORRELATION_DETAIL_KEY: {"version": 99},
        })),
        is_error: false,
    };
    assert_eq!(
        state.on_tool_finished("malformed", "codebase_memory_search_graph", &malformed),
        DecisionAnchorTransition::Unchanged
    );
    assert!(!state.blocks_mutation("write"));
}

#[test]
fn recovery_stages_implementation_before_its_caller_traversal() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    install_consumable_root(&mut state);
    enter_budget_recovery(&mut state, 1);

    assert_eq!(
        state.recovery_details().unwrap().compatible_actions,
        [GraphRecoveryActionV1::for_evidence(
            GraphRecoveryEvidenceKindV1::Implementation,
        )]
    );
    assert_eq!(
        state.on_tool_dispatched(
            &source_call("implementation", DecisionEvidenceKindV1::Implementation),
            3,
        ),
        None,
    );
    assert_eq!(
        finish_with_evidence(
            &mut state,
            "implementation",
            ROOT,
            DecisionEvidenceKindV1::Implementation,
        ),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );
    assert_eq!(
        state.recovery_details().unwrap().compatible_actions,
        [GraphRecoveryActionV1::for_evidence(
            GraphRecoveryEvidenceKindV1::Trace,
        )]
    );

    assert_eq!(
        state.on_tool_dispatched(&call("trace", "codebase_memory_trace_path"), 4),
        None,
    );
    assert_eq!(
        state.on_tool_finished(
            "trace",
            "codebase_memory_trace_path",
            &output(
                "codebase_memory_trace_path",
                ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
            ),
        ),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );
    assert_eq!(
        state.on_tool_dispatched(&source_call("caller", DecisionEvidenceKindV1::Caller), 5),
        None,
    );
    assert_eq!(
        finish_with_evidence(
            &mut state,
            "caller",
            ROOT,
            DecisionEvidenceKindV1::Caller,
        ),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );
    assert_eq!(
        finish_semantic_test_search(
            &mut state,
            "semantic-test-search",
            ROOT,
            6,
            FocusedTestDiscoveryOutcomeV1::EligibleSelectorReturned,
        ),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );
    let mut test = source_call("test", DecisionEvidenceKindV1::FocusedTest);
    test.arguments["qualified_name"] = serde_json::json!("semantic-returned-test");
    assert_eq!(state.on_tool_dispatched(&test, 7), None);
    assert_eq!(
        finish_with_evidence(
            &mut state,
            "test",
            ROOT,
            DecisionEvidenceKindV1::FocusedTest,
        ),
        DecisionAnchorTransition::Converged,
    );
    assert!(state.blocks_mutation("write"));
}

#[test]
fn allowance_floor_keeps_a_distinct_provider_selector_actionable() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    install_consumable_root(&mut state);
    enter_budget_recovery(&mut state, 1);
    let admission = EligibleLineageAdmission::new(
        ROOT.to_string(),
        DecisionAnchorTargetKindV1::QualifiedName,
        GraphCorrelationToolV1::GetCodeSnippet,
        Some(DecisionEvidenceKindV1::Implementation),
    )
    .unwrap();

    for index in 0..MAX_GRAPH_RECOVERY_ALLOWANCE_V1 {
        let id = format!("non-progress-{index}");
        let mut attempted = source_call(&id, DecisionEvidenceKindV1::Implementation);
        attempted.arguments["qualified_name"] =
            serde_json::json!(format!("alternative-{index}"));
        assert_eq!(
            state.on_tool_dispatched_with_admission(
                &attempted,
                usize::from(index) + 3,
                Some(&LineageAdmissionOutcome::Eligible(admission.clone())),
            ),
            None,
        );
        assert_eq!(
            state.on_tool_finished(
                &id,
                "codebase_memory_get_code_snippet",
                &output_with_evidence(
                    ROOT,
                    DecisionAnchorLineageStageV1::CarryForward,
                    DecisionEvidenceKindV1::Caller,
                ),
            ),
            DecisionAnchorTransition::GapRecoveryNeeded,
        );
    }

    let details = state.recovery_details().expect("alternative remains actionable");
    assert_eq!(details.remaining_allowance, 1);
    assert_eq!(
        details.compatible_actions,
        [GraphRecoveryActionV1::for_evidence(
            GraphRecoveryEvidenceKindV1::Implementation,
        )]
    );
    assert_eq!(
        state.on_tool_dispatched_with_admission(
            &source_call(
                "eligible-alternative",
                DecisionEvidenceKindV1::Implementation,
            ),
            7,
            Some(&LineageAdmissionOutcome::Eligible(admission)),
        ),
        None,
    );
}

#[test]
fn recovery_without_an_implementation_source_releases_exact_read_bounded_fallback() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    state.on_tool_dispatched(&call("root", "codebase_memory_search_graph"), 0);
    assert_eq!(
        finish_with_kinds(
            &mut state,
            "root",
            "codebase_memory_search_graph",
            ROOT,
            DecisionAnchorLineageStageV1::Root,
            &[DecisionAnchorTargetKindV1::FunctionName],
        ),
        DecisionAnchorTransition::Unchanged,
    );
    state.on_tool_dispatched(&call("broad-one", "codebase_memory_get_architecture"), 1);
    assert_eq!(
        state.on_tool_finished(
            "broad-one",
            "codebase_memory_get_architecture",
            &plain_success(),
        ),
        DecisionAnchorTransition::Unchanged,
    );
    state.on_tool_dispatched(&call("broad-two", "codebase_memory_get_architecture"), 2);
    assert_eq!(
        state.on_tool_finished(
            "broad-two",
            "codebase_memory_get_architecture",
            &plain_success(),
        ),
        DecisionAnchorTransition::ConventionalFallbackReleased,
    );
    assert!(state.blocks_mutation("write"));
    assert_eq!(
        state.on_tool_dispatched(
            &source_call("unsupported-source", DecisionEvidenceKindV1::Implementation),
            3,
        ),
        conventional_fallback_graph_denial(),
    );
    assert_eq!(
        state.on_tool_dispatched(&call("unsupported-trace", "codebase_memory_trace_path"), 4),
        conventional_fallback_graph_denial(),
    );
}

#[test]
fn expected_unavailable_gap_releases_fallback_without_reopening_graph() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    install_consumable_root(&mut state);
    state.on_tool_dispatched(&call("trace", "codebase_memory_trace_path"), 1);
    finish(
        &mut state,
        "trace",
        "codebase_memory_trace_path",
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
    );
    for (turn, id, kind) in [
        (2, "implementation", DecisionEvidenceKindV1::Implementation),
        (3, "test", DecisionEvidenceKindV1::FocusedTest),
    ] {
        state.on_tool_dispatched(&source_call(id, kind), turn);
        finish_with_evidence(&mut state, id, ROOT, kind);
    }
    enter_budget_recovery(&mut state, 4);

    assert_eq!(
        state.on_tool_dispatched(&source_call("caller", DecisionEvidenceKindV1::Caller), 6),
        None,
    );
    assert_eq!(
        state.on_tool_finished(
            "caller",
            "codebase_memory_get_code_snippet",
            &failure_output("transport"),
        ),
        DecisionAnchorTransition::ConventionalFallbackReleased,
    );
    assert!(state.blocks_mutation("write"));
    assert_eq!(
        state.on_tool_dispatched(&source_call("retry", DecisionEvidenceKindV1::Caller), 7),
        conventional_fallback_graph_denial(),
    );
}

#[test]
fn read_only_roles_retain_the_same_staged_bounded_gap_path() {
    let read_only_effects = effects()
        .into_iter()
        .filter(|(_, effect)| !effect.writes)
        .collect::<BTreeMap<_, _>>();
    let mut state = DecisionAnchorState::from_effects(&read_only_effects).unwrap();
    install_consumable_root(&mut state);
    enter_budget_recovery(&mut state, 1);

    for (turn, id, kind, expected) in [
        (
            3,
            "implementation",
            DecisionEvidenceKindV1::Implementation,
            DecisionAnchorTransition::GapRecoveryNeeded,
        ),
    ] {
        assert_eq!(state.on_tool_dispatched(&source_call(id, kind), turn), None);
        assert_eq!(finish_with_evidence(&mut state, id, ROOT, kind), expected);
    }
    assert_eq!(
        state.on_tool_dispatched(&call("trace", "codebase_memory_trace_path"), 4),
        None,
    );
    assert_eq!(
        state.on_tool_finished(
            "trace",
            "codebase_memory_trace_path",
            &output(
                "codebase_memory_trace_path",
                ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
            ),
        ),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );
    assert_eq!(
        state.on_tool_dispatched(&source_call("caller", DecisionEvidenceKindV1::Caller), 5),
        None,
    );
    assert_eq!(
        finish_with_evidence(
            &mut state,
            "caller",
            ROOT,
            DecisionEvidenceKindV1::Caller,
        ),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );
    assert_eq!(
        finish_semantic_test_search(
            &mut state,
            "semantic-test-search",
            ROOT,
            6,
            FocusedTestDiscoveryOutcomeV1::EligibleSelectorReturned,
        ),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );
    let mut test = source_call("test", DecisionEvidenceKindV1::FocusedTest);
    test.arguments["qualified_name"] = serde_json::json!("semantic-returned-test");
    assert_eq!(state.on_tool_dispatched(&test, 7), None);
    assert_eq!(
        finish_with_evidence(
            &mut state,
            "test",
            ROOT,
            DecisionEvidenceKindV1::FocusedTest,
        ),
        DecisionAnchorTransition::Converged,
    );
    assert_eq!(state.on_tool_dispatched(&call("ordinary", "read"), 7), None);
}

#[test]
fn broad_recovery_denial_retains_exact_missing_caller_evidence() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    install_consumable_root(&mut state);
    state.on_tool_dispatched(&call("trace", "codebase_memory_trace_path"), 1);
    finish(
        &mut state,
        "trace",
        "codebase_memory_trace_path",
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
    );
    for (turn, id, kind) in [
        (2, "implementation", DecisionEvidenceKindV1::Implementation),
        (3, "test", DecisionEvidenceKindV1::FocusedTest),
    ] {
        state.on_tool_dispatched(&source_call(id, kind), turn);
        finish_with_evidence(&mut state, id, ROOT, kind);
    }
    enter_budget_recovery(&mut state, 4);

    let guidance = state.recovery_details().expect("recovery guidance");
    assert_eq!(
        guidance.compatible_actions,
        [GraphRecoveryActionV1::for_evidence(
            GraphRecoveryEvidenceKindV1::Caller,
        )]
    );

    let denial = state.on_tool_dispatched_with_admission(
        &call(
            "recovery-broad-architecture-denied",
            "codebase_memory_get_architecture",
        ),
        6,
        Some(&LineageAdmissionOutcome::Ineligible(
            crate::LineageAdmissionStatus::UnsupportedTool,
        )),
    );
    let exact = GraphExplorationClosedV1::recoverable_without_actions(
        [GraphRecoveryEvidenceKindV1::Caller],
        4,
    )
    .expect("exact local denial");
    assert_eq!(
        exact.model_message(),
        "decision-evidence recovery required; missing evidence: [caller]; permitted action: targeted_current_root_graph_call; remaining allowance: 4"
    );
    assert_eq!(
        serde_json::to_value(&exact).unwrap(),
        serde_json::json!({
            "reason": "recoverable_incomplete_evidence",
            "missing_evidence": ["caller"],
            "permitted_action": "targeted_current_root_graph_call",
            "remaining_allowance": 4,
        })
    );
    assert_eq!(
        denial,
        Some(ToolCallDenial::GraphExplorationClosed(Some(exact)))
    );

    let caller_admission = EligibleLineageAdmission::new(
        ROOT.to_string(),
        DecisionAnchorTargetKindV1::QualifiedName,
        GraphCorrelationToolV1::GetCodeSnippet,
        Some(DecisionEvidenceKindV1::Caller),
    )
    .expect("closed caller admission");
    assert_eq!(
        state.on_tool_dispatched_with_admission(
            &source_call("caller", DecisionEvidenceKindV1::Caller),
            7,
            Some(&LineageAdmissionOutcome::Eligible(caller_admission)),
        ),
        None,
    );
    assert_eq!(
        finish_with_evidence(
            &mut state,
            "caller",
            ROOT,
            DecisionEvidenceKindV1::Caller,
        ),
        DecisionAnchorTransition::Converged,
    );
}

#[test]
fn budget_exhaustion_queues_exact_actionable_missing_evidence_guidance() {
    let mut machine = AgentMachine::with_effects(vec![user("repair")], 10, effects());
    let _ = machine.on_start(EngineTime::ZERO);
    let _ = complete(
        &mut machine,
        llm_responded(assistant_tool_calls(&[("root", "codebase_memory_search_graph")])),
    );
    let _ = complete(
        &mut machine,
        tool_finished(
            "root",
            output(
                "codebase_memory_search_graph",
                ROOT,
                DecisionAnchorLineageStageV1::Root,
            ),
        ),
    );

    let mut recovery_requests = Vec::new();
    for id in ["broad-one", "broad-two"] {
        let _ = complete(
            &mut machine,
            llm_responded(assistant_tool_calls(&[(
                id,
                "codebase_memory_get_architecture",
            )])),
        );
        recovery_requests = complete(&mut machine, tool_finished(id, plain_success()));
    }

    assert!(message_containing(
        &recovery_requests,
        "missing evidence: [trace, implementation, caller, focused_test]"
    ));
    assert!(message_containing(
        &recovery_requests,
        "permitted action: targeted_current_root_graph_call; remaining allowance: 4"
    ));
    assert!(message_containing(
        &recovery_requests,
        "compatible actions: [get_code_snippet/qualified_name/implementation]"
    ));
    assert!(message_containing(
        &recovery_requests,
        "use only these current-root actions"
    ));
    assert!(message_containing(
        &recovery_requests,
        "no retry, root switch, or mutation"
    ));
    assert!(!message_containing(
        &recovery_requests,
        DECISION_ANCHOR_RECOVERY_MESSAGE
    ));
    assert!(!message_containing(&recovery_requests, ROOT));
    assert!(!message_containing(&recovery_requests, OTHER_ROOT));
}

#[test]
fn no_compatible_implementation_source_releases_fallback_before_another_graph_loop() {
    let mut machine = AgentMachine::with_effects(vec![user("repair")], 10, effects());
    let _ = machine.on_start(EngineTime::ZERO);
    let _ = complete(
        &mut machine,
        llm_responded(assistant_tool_calls(&[("root", "codebase_memory_search_graph")])),
    );
    let _ = complete(
        &mut machine,
        tool_finished(
            "root",
            output_with_kinds(
                "codebase_memory_search_graph",
                ROOT,
                DecisionAnchorLineageStageV1::Root,
                &[DecisionAnchorTargetKindV1::FunctionName],
            ),
        ),
    );
    let mut fallback = Vec::new();
    for id in ["broad-one", "broad-two"] {
        let _ = complete(
            &mut machine,
            llm_responded(assistant_tool_calls(&[(
                id,
                "codebase_memory_get_architecture",
            )])),
        );
        fallback = complete(&mut machine, tool_finished(id, plain_success()));
    }
    assert_eq!(calls_llm(&fallback), 1);
    assert!(message_containing(
        &fallback,
        DECISION_ANCHOR_CONVENTIONAL_FALLBACK_MESSAGE,
    ));
    assert!(!machine.is_stopped());
}
