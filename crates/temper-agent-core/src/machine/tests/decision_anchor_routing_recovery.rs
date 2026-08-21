// Routing benchmark ordering regression for decision-evidence convergence.

use super::*;

#[test]
fn caller_source_after_duplicate_refinement_completes_parallel_root_evidence() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    state.on_tool_dispatched(&call("routing-root", "codebase_memory_search_graph"), 0);
    state.on_tool_dispatched(&call("test-root", "codebase_memory_search_graph"), 0);
    let routing_root = output(
        "codebase_memory_search_graph",
        ROOT,
        DecisionAnchorLineageStageV1::Root,
    );
    let test_root = output(
        "codebase_memory_search_graph",
        OTHER_ROOT,
        DecisionAnchorLineageStageV1::Root,
    );
    assert_eq!(
        state.on_tool_batch_finished(&[
            (
                "routing-root",
                "codebase_memory_search_graph",
                &routing_root,
            ),
            ("test-root", "codebase_memory_search_graph", &test_root),
        ]),
        DecisionAnchorTransition::Unchanged,
    );

    state.on_tool_dispatched(&call("implementation", "codebase_memory_search_code"), 1);
    state.on_tool_dispatched(&call("caller-trace", "codebase_memory_trace_path"), 1);
    state.on_tool_dispatched(
        &source_call("focused-test", DecisionEvidenceKindV1::FocusedTest),
        1,
    );
    let implementation = output(
        "codebase_memory_search_code",
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
    );
    let caller_trace = output(
        "codebase_memory_trace_path",
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
    );
    let focused_test = output_with_evidence(
        OTHER_ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
        DecisionEvidenceKindV1::FocusedTest,
    );
    assert_eq!(
        state.on_tool_batch_finished(&[
            (
                "implementation",
                "codebase_memory_search_code",
                &implementation,
            ),
            (
                "caller-trace",
                "codebase_memory_trace_path",
                &caller_trace,
            ),
            (
                "focused-test",
                "codebase_memory_get_code_snippet",
                &focused_test,
            ),
        ]),
        DecisionAnchorTransition::Unchanged,
    );
    assert!(state.blocks_mutation("write"));

    state.on_tool_dispatched(&call("duplicate", "codebase_memory_search_code"), 2);
    assert_eq!(
        state.on_tool_finished(
            "duplicate",
            "codebase_memory_search_code",
            &implementation,
        ),
        DecisionAnchorTransition::Unchanged,
    );
    assert!(state.blocks_mutation("write"));

    state.on_tool_dispatched(&source_call("caller", DecisionEvidenceKindV1::Caller), 3);
    assert_eq!(
        finish_with_evidence(&mut state, "caller", ROOT, DecisionEvidenceKindV1::Caller),
        DecisionAnchorTransition::Converged,
    );
    assert!(!state.blocks_mutation("write"));
    assert_eq!(
        state.on_tool_dispatched(&call("closed", "codebase_memory_search_graph"), 4),
        Some(ToolCallDenial::GraphExplorationClosed),
    );
    assert_eq!(state.on_tool_dispatched(&call("mutation", "write"), 4), None);
}
