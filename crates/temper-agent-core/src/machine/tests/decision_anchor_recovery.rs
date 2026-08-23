// Decision-anchor recovery and malformed-result regressions.

use super::*;
use crate::{EligibleLineageAdmission, LineageAdmissionOutcome};

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
fn missing_trace_can_advance_before_the_typed_source_gaps() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    install_consumable_root(&mut state);
    enter_budget_recovery(&mut state, 1);

    assert_eq!(
        state.on_tool_dispatched(&call("trace", "codebase_memory_trace_path"), 3),
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
    assert!(state.blocks_mutation("write"));

    for (id, kind) in [
        ("implementation", DecisionEvidenceKindV1::Implementation),
        ("caller", DecisionEvidenceKindV1::Caller),
        ("test", DecisionEvidenceKindV1::FocusedTest),
    ] {
        assert_eq!(state.on_tool_dispatched(&source_call(id, kind), 4), None);
    }
    let implementation = output_with_evidence(
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
        DecisionEvidenceKindV1::Implementation,
    );
    let caller = output_with_evidence(
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
        DecisionEvidenceKindV1::Caller,
    );
    let test = output_with_evidence(
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
        DecisionEvidenceKindV1::FocusedTest,
    );
    assert_eq!(
        state.on_tool_batch_finished(&[
            (
                "implementation",
                "codebase_memory_get_code_snippet",
                &implementation,
            ),
            ("caller", "codebase_memory_get_code_snippet", &caller),
            ("test", "codebase_memory_get_code_snippet", &test),
        ]),
        DecisionAnchorTransition::Converged,
    );
    assert!(!state.blocks_mutation("write"));
}

#[test]
fn recovery_denies_unsupported_gap_and_stops_when_last_path_depletes() {
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
    state.on_tool_dispatched(
        &call("broad-one", "codebase_memory_get_architecture"),
        1,
    );
    state.on_tool_finished(
        "broad-one",
        "codebase_memory_get_architecture",
        &plain_success(),
    );
    state.on_tool_dispatched(
        &call("broad-two", "codebase_memory_get_architecture"),
        2,
    );
    assert_eq!(
        state.on_tool_finished(
            "broad-two",
            "codebase_memory_get_architecture",
            &plain_success(),
        ),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );
    assert_eq!(
        state.on_tool_dispatched(
            &source_call("unsupported-source", DecisionEvidenceKindV1::Implementation),
            3,
        ),
        recovery_graph_denial(all_missing(), 4),
        "a source selector absent from the current root cannot consume recovery allowance",
    );
    assert_eq!(
        state.on_tool_dispatched(&call("trace", "codebase_memory_trace_path"), 3),
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
        DecisionAnchorTransition::RecoveryExhausted,
        "once the only supported gap is filled, impossible remaining gaps terminate recovery",
    );
    assert!(state.blocks_mutation("write"));
    assert_eq!(
        state.on_tool_dispatched(&call("later", "codebase_memory_trace_path"), 4),
        exhausted_graph_denial([
            GraphRecoveryEvidenceKindV1::Implementation,
            GraphRecoveryEvidenceKindV1::Caller,
            GraphRecoveryEvidenceKindV1::FocusedTest,
        ]),
    );
}

#[test]
fn each_missing_typed_purpose_can_complete_after_budget_exhaustion() {
    let required = [
        DecisionEvidenceKindV1::Implementation,
        DecisionEvidenceKindV1::Caller,
        DecisionEvidenceKindV1::FocusedTest,
    ];
    for missing in required {
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
        let mut turn = 2;
        let mut satisfied = None;
        for kind in required.into_iter().filter(|kind| *kind != missing) {
            let id = format!("satisfied-{turn}");
            state.on_tool_dispatched(&source_call(&id, kind), turn);
            finish_with_evidence(&mut state, &id, ROOT, kind);
            satisfied = Some(kind);
            turn += 1;
        }
        enter_budget_recovery(&mut state, turn);

        assert_eq!(
            state.on_tool_dispatched(&call("duplicate-trace", "codebase_memory_trace_path"), turn + 2),
            recovery_graph_denial(
                [match missing {
                    DecisionEvidenceKindV1::Implementation => GraphRecoveryEvidenceKindV1::Implementation,
                    DecisionEvidenceKindV1::Caller => GraphRecoveryEvidenceKindV1::Caller,
                    DecisionEvidenceKindV1::FocusedTest => GraphRecoveryEvidenceKindV1::FocusedTest,
                }],
                4,
            ),
        );
        assert_eq!(
            state.on_tool_dispatched(
                &source_call("satisfied", satisfied.expect("two purposes were installed")),
                turn + 2,
            ),
            recovery_graph_denial(
                [match missing {
                    DecisionEvidenceKindV1::Implementation => GraphRecoveryEvidenceKindV1::Implementation,
                    DecisionEvidenceKindV1::Caller => GraphRecoveryEvidenceKindV1::Caller,
                    DecisionEvidenceKindV1::FocusedTest => GraphRecoveryEvidenceKindV1::FocusedTest,
                }],
                4,
            ),
        );
        assert_eq!(
            state.on_tool_dispatched(&source_call("missing", missing), turn + 2),
            None,
        );
        assert_eq!(
            finish_with_evidence(&mut state, "missing", ROOT, missing),
            DecisionAnchorTransition::Converged,
        );
        assert!(!state.blocks_mutation("write"));
    }
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
        DecisionAnchorTransition::Unchanged,
    );
    assert!(!state.blocks_mutation("write"));
    assert_eq!(
        state.on_tool_dispatched(&source_call("retry", DecisionEvidenceKindV1::Caller), 7),
        legacy_graph_denial(),
    );
}

#[test]
fn read_only_roles_retain_the_same_bounded_gap_path() {
    let read_only_effects = effects()
        .into_iter()
        .filter(|(_, effect)| !effect.writes)
        .collect::<BTreeMap<_, _>>();
    let mut state = DecisionAnchorState::from_effects(&read_only_effects).unwrap();
    install_consumable_root(&mut state);
    enter_budget_recovery(&mut state, 1);

    assert_eq!(
        state.on_tool_dispatched(&call("broad", "codebase_memory_search_graph"), 3),
        recovery_graph_denial(all_missing(), 4),
    );
    assert_eq!(
        state.on_tool_dispatched(&call("trace", "codebase_memory_trace_path"), 3),
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
    for (id, kind) in [
        ("implementation", DecisionEvidenceKindV1::Implementation),
        ("caller", DecisionEvidenceKindV1::Caller),
        ("test", DecisionEvidenceKindV1::FocusedTest),
    ] {
        assert_eq!(state.on_tool_dispatched(&source_call(id, kind), 4), None);
    }
    let implementation = output_with_evidence(
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
        DecisionEvidenceKindV1::Implementation,
    );
    let caller = output_with_evidence(
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
        DecisionEvidenceKindV1::Caller,
    );
    let test = output_with_evidence(
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
        DecisionEvidenceKindV1::FocusedTest,
    );
    assert_eq!(
        state.on_tool_batch_finished(&[
            ("test", "codebase_memory_get_code_snippet", &test),
            ("caller", "codebase_memory_get_code_snippet", &caller),
            (
                "implementation",
                "codebase_memory_get_code_snippet",
                &implementation,
            ),
        ]),
        DecisionAnchorTransition::Converged,
    );
    assert_eq!(state.on_tool_dispatched(&call("ordinary", "read"), 4), None);
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
        "compatible actions: [trace_path/function_name/trace]"
    ));
    assert!(!message_containing(
        &recovery_requests,
        DECISION_ANCHOR_RECOVERY_MESSAGE
    ));
}

#[test]
fn no_compatible_action_stops_before_another_recovery_loop() {
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
    for id in ["broad-one", "broad-two"] {
        let _ = complete(
            &mut machine,
            llm_responded(assistant_tool_calls(&[(
                id,
                "codebase_memory_get_architecture",
            )])),
        );
        let _ = complete(&mut machine, tool_finished(id, plain_success()));
    }

    let requests = complete(
        &mut machine,
        llm_responded(assistant_tool_calls(&[("trace", "codebase_memory_trace_path")])),
    );
    assert!(requests.iter().any(|request| {
        matches!(
            request,
            AgentRequest::RunTool {
                call,
                denial: None,
                ..
            } if call.id == "trace"
        )
    }));
    let stopped = complete(
        &mut machine,
        tool_finished(
            "trace",
            output(
                "codebase_memory_trace_path",
                ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
            ),
        ),
    );
    assert_eq!(
        final_stop(&stopped),
        Some(crate::machine::AgentStop::DecisionAnchorRecoveryExhausted),
    );
    assert!(machine.is_stopped());
}
