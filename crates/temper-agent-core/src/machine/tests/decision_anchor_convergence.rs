// Progress-bounded convergence regressions shared with the decision-anchor fixtures.

use super::*;

#[test]
fn read_only_roles_converge_and_close_graph_tools_without_mutation_effects() {
    let read_only_effects = effects()
        .into_iter()
        .filter(|(_, effect)| !effect.writes)
        .collect::<BTreeMap<_, _>>();
    let mut state = DecisionAnchorState::from_effects(&read_only_effects)
        .expect("graph-enabled read-only roles install the per-run guard");
    state.on_tool_dispatched(&call("root", "codebase_memory_search_graph"), 0);
    finish(
        &mut state,
        "root",
        "codebase_memory_search_graph",
        ROOT,
        DecisionAnchorLineageStageV1::Root,
    );
    for (id, name, evidence_kind) in [
        ("trace", "codebase_memory_trace_path", None),
        (
            "implementation",
            "codebase_memory_get_code_snippet",
            Some(DecisionEvidenceKindV1::Implementation),
        ),
        (
            "caller",
            "codebase_memory_get_code_snippet",
            Some(DecisionEvidenceKindV1::Caller),
        ),
        (
            "test",
            "codebase_memory_get_code_snippet",
            Some(DecisionEvidenceKindV1::FocusedTest),
        ),
    ] {
        let call = evidence_kind.map_or_else(|| call(id, name), |kind| source_call(id, kind));
        state.on_tool_dispatched(&call, 1);
    }
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
            ("trace", "codebase_memory_trace_path", &trace),
            (
                "implementation",
                "codebase_memory_get_code_snippet",
                &implementation,
            ),
            ("caller", "codebase_memory_get_code_snippet", &caller),
            ("test", "codebase_memory_get_code_snippet", &test),
        ]),
        DecisionAnchorTransition::Converged
    );
    assert_eq!(
        state.on_tool_dispatched(&call("extra", "codebase_memory_search_graph"), 2),
        Some(ToolCallDenial::GraphExplorationClosed)
    );
    assert_eq!(
        state.on_tool_dispatched(&call("conventional", "read"), 2),
        None,
        "conventional reads remain available after convergence"
    );
}

#[test]
fn repeated_non_progressing_discovery_closes_graph_exploration() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    for (turn, id, expected) in [
        (0, "broad-one", DecisionAnchorTransition::Unchanged),
        (
            1,
            "broad-two",
            DecisionAnchorTransition::ExplorationExhausted,
        ),
    ] {
        state.on_tool_dispatched(&call(id, "codebase_memory_get_architecture"), turn);
        assert_eq!(
            state.on_tool_finished(id, "codebase_memory_get_architecture", &plain_success()),
            expected
        );
    }
    assert_eq!(
        state.on_tool_dispatched(&call("broad-three", "codebase_memory_get_architecture"), 2),
        Some(ToolCallDenial::GraphExplorationClosed)
    );
    assert_eq!(
        state.on_tool_dispatched(&call("conventional", "read"), 2),
        None
    );
}

#[test]
fn typed_progress_resets_the_open_non_progress_budget() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    state.on_tool_dispatched(&call("root", "codebase_memory_search_graph"), 0);
    finish(
        &mut state,
        "root",
        "codebase_memory_search_graph",
        ROOT,
        DecisionAnchorLineageStageV1::Root,
    );

    state.on_tool_dispatched(
        &call("broad-before-trace", "codebase_memory_get_architecture"),
        1,
    );
    assert_eq!(
        state.on_tool_finished(
            "broad-before-trace",
            "codebase_memory_get_architecture",
            &plain_success(),
        ),
        DecisionAnchorTransition::Unchanged,
    );

    state.on_tool_dispatched(&call("trace", "codebase_memory_trace_path"), 2);
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
        DecisionAnchorTransition::Unchanged,
    );

    state.on_tool_dispatched(
        &call("broad-after-trace", "codebase_memory_get_architecture"),
        3,
    );
    assert_eq!(
        state.on_tool_finished(
            "broad-after-trace",
            "codebase_memory_get_architecture",
            &plain_success(),
        ),
        DecisionAnchorTransition::Unchanged,
        "successful typed lineage progress starts a fresh bounded exploration window",
    );
    state.on_tool_dispatched(
        &call("broad-exhausts", "codebase_memory_get_architecture"),
        4,
    );
    assert_eq!(
        state.on_tool_finished(
            "broad-exhausts",
            "codebase_memory_get_architecture",
            &plain_success(),
        ),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );
}
