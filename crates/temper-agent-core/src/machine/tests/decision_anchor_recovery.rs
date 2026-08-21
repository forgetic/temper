// Decision-anchor recovery and malformed-result regressions.

use super::*;

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
fn exhausted_broad_search_admits_only_parallel_calls_for_named_missing_gaps() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    install_consumable_root(&mut state);
    enter_budget_recovery(&mut state, 1);

    assert!(state.blocks_mutation("write"));
    assert_eq!(
        state.on_tool_dispatched(&call("blocked-write", "write"), 3),
        Some(ToolCallDenial::DecisionAnchorMutation),
    );
    for (id, name) in [
        ("broad", "codebase_memory_search_graph"),
        ("refinement", "codebase_memory_search_code"),
        ("undeclared", "codebase_memory_get_code_snippet"),
    ] {
        assert_eq!(
            state.on_tool_dispatched(&call(id, name), 3),
            Some(ToolCallDenial::GraphExplorationClosed),
        );
    }
    assert_eq!(state.on_tool_dispatched(&call("ordinary", "read"), 3), None);

    assert_eq!(
        state.on_tool_dispatched(&call("trace", "codebase_memory_trace_path"), 3),
        None,
    );
    assert_eq!(
        state.on_tool_dispatched(
            &source_call("implementation", DecisionEvidenceKindV1::Implementation),
            3,
        ),
        None,
    );
    assert_eq!(
        state.on_tool_dispatched(
            &source_call("duplicate", DecisionEvidenceKindV1::Implementation),
            3,
        ),
        Some(ToolCallDenial::GraphExplorationClosed),
        "a pending purpose cannot consume a second recovery slot",
    );
    assert_eq!(
        state.on_tool_dispatched(&source_call("caller", DecisionEvidenceKindV1::Caller), 3),
        None,
    );
    assert_eq!(
        state.on_tool_dispatched(
            &source_call("test", DecisionEvidenceKindV1::FocusedTest),
            3,
        ),
        None,
    );

    let trace = output(
        "codebase_memory_trace_path",
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
    );
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
            ("implementation", "codebase_memory_get_code_snippet", &implementation),
            ("trace", "codebase_memory_trace_path", &trace),
            ("caller", "codebase_memory_get_code_snippet", &caller),
            ("test", "codebase_memory_get_code_snippet", &test),
        ]),
        DecisionAnchorTransition::Converged,
    );
    assert!(!state.blocks_mutation("write"));
    assert_eq!(
        state.on_tool_dispatched(&call("closed", "codebase_memory_trace_path"), 4),
        Some(ToolCallDenial::GraphExplorationClosed),
    );
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
        Some(ToolCallDenial::GraphExplorationClosed),
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
        Some(ToolCallDenial::GraphExplorationClosed),
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
            Some(ToolCallDenial::GraphExplorationClosed),
        );
        assert_eq!(
            state.on_tool_dispatched(
                &source_call("satisfied", satisfied.expect("two purposes were installed")),
                turn + 2,
            ),
            Some(ToolCallDenial::GraphExplorationClosed),
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
        Some(ToolCallDenial::GraphExplorationClosed),
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
        Some(ToolCallDenial::GraphExplorationClosed),
    );
    for (id, kind) in [
        ("implementation", DecisionEvidenceKindV1::Implementation),
        ("caller", DecisionEvidenceKindV1::Caller),
        ("test", DecisionEvidenceKindV1::FocusedTest),
    ] {
        assert_eq!(state.on_tool_dispatched(&source_call(id, kind), 3), None);
    }
    assert_eq!(
        state.on_tool_dispatched(&call("trace", "codebase_memory_trace_path"), 3),
        None,
    );
    let trace = output(
        "codebase_memory_trace_path",
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
    );
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
            ("trace", "codebase_memory_trace_path", &trace),
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
fn depleted_gap_recovery_stops_the_machine_after_cross_root_malformed_and_later_roots() {
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

    let mut recovery_calls = assistant_tool_calls(&[
        ("trace", "codebase_memory_trace_path"),
        ("implementation", "codebase_memory_get_code_snippet"),
        ("caller", "codebase_memory_get_code_snippet"),
        ("test", "codebase_memory_get_code_snippet"),
    ]);
    for block in &mut recovery_calls.content {
        let ContentBlock::ToolCall(call) = block else {
            continue;
        };
        call.arguments = match call.id.as_str() {
            "implementation" => serde_json::json!({"decision_evidence_kind": "implementation"}),
            "caller" => serde_json::json!({"decision_evidence_kind": "caller"}),
            "test" => serde_json::json!({"decision_evidence_kind": "focused_test"}),
            _ => serde_json::json!({}),
        };
    }
    let requests = complete(&mut machine, llm_responded(recovery_calls));
    assert_eq!(run_tools(&requests), ["trace", "implementation", "caller", "test"]);

    assert!(
        complete(
            &mut machine,
            tool_finished(
                "trace",
                output(
                    "codebase_memory_trace_path",
                    OTHER_ROOT,
                    DecisionAnchorLineageStageV1::CarryForward,
                ),
            ),
        )
        .is_empty()
    );
    assert!(
        complete(
            &mut machine,
            tool_finished(
                "implementation",
                output_with_evidence(
                    ROOT,
                    DecisionAnchorLineageStageV1::Root,
                    DecisionEvidenceKindV1::Implementation,
                ),
            ),
        )
        .is_empty()
    );
    assert!(complete(&mut machine, tool_finished("caller", plain_success())).is_empty());
    let stopped = complete(
        &mut machine,
        tool_finished(
            "test",
            output_with_evidence(
                OTHER_ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
                DecisionEvidenceKindV1::FocusedTest,
            ),
        ),
    );
    assert_eq!(
        final_stop(&stopped),
        Some(crate::machine::AgentStop::DecisionAnchorRecoveryExhausted),
    );
    assert!(machine.is_stopped());
}
