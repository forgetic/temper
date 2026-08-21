// Typed evidence completion and mutation-gating regressions.

use super::*;

#[test]
fn direct_trace_and_typed_current_root_sources_complete_without_search_code() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    state.on_tool_dispatched(&call("root", "codebase_memory_search_graph"), 0);
    finish(
        &mut state,
        "root",
        "codebase_memory_search_graph",
        ROOT,
        DecisionAnchorLineageStageV1::Root,
    );

    state.on_tool_dispatched(&call("trace", "codebase_memory_trace_path"), 1);
    state.on_tool_dispatched(
        &source_call("implementation", DecisionEvidenceKindV1::Implementation),
        1,
    );
    state.on_tool_dispatched(&source_call("caller", DecisionEvidenceKindV1::Caller), 1);
    state.on_tool_dispatched(
        &source_call("behavior", DecisionEvidenceKindV1::FocusedTest),
        1,
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
    let behavior = output_with_evidence(
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
            ("trace", "codebase_memory_trace_path", &trace),
            ("caller", "codebase_memory_get_code_snippet", &caller),
            ("behavior", "codebase_memory_get_code_snippet", &behavior),
        ]),
        DecisionAnchorTransition::Converged,
    );
    assert!(
        !state.blocks_mutation("write"),
        "the trace and every typed current-root source purpose complete evidence"
    );
}

#[test]
fn parallel_sources_then_trace_batch_completes_the_same_evidence_set() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    state.on_tool_dispatched(&call("root", "codebase_memory_search_graph"), 0);
    finish(
        &mut state,
        "root",
        "codebase_memory_search_graph",
        ROOT,
        DecisionAnchorLineageStageV1::Root,
    );

    // Pre-trace snippets use the same settled pre-batch root as trace-first batches.
    state.on_tool_dispatched(
        &source_call("implementation", DecisionEvidenceKindV1::Implementation),
        1,
    );
    state.on_tool_dispatched(&call("trace", "codebase_memory_trace_path"), 1);
    state.on_tool_dispatched(&source_call("caller", DecisionEvidenceKindV1::Caller), 1);
    state.on_tool_dispatched(
        &source_call("behavior", DecisionEvidenceKindV1::FocusedTest),
        1,
    );
    let implementation = output_with_evidence(
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
        DecisionEvidenceKindV1::Implementation,
    );
    let trace = output(
        "codebase_memory_trace_path",
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
    );
    let caller = output_with_evidence(
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
        DecisionEvidenceKindV1::Caller,
    );
    let behavior = output_with_evidence(
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
            ("trace", "codebase_memory_trace_path", &trace),
            ("caller", "codebase_memory_get_code_snippet", &caller),
            ("behavior", "codebase_memory_get_code_snippet", &behavior),
        ]),
        DecisionAnchorTransition::Converged,
    );
    assert!(
        !state.blocks_mutation("write"),
        "sibling dispatch order cannot change complete current-root evidence"
    );
}

#[test]
fn root_producer_and_same_turn_dependents_stay_ineligible() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    state.on_tool_dispatched(&call("root", "codebase_memory_search_graph"), 0);
    state.on_tool_dispatched(&call("trace", "codebase_memory_trace_path"), 0);
    state.on_tool_dispatched(&call("source-one", "codebase_memory_get_code_snippet"), 0);
    state.on_tool_dispatched(&call("source-two", "codebase_memory_get_code_snippet"), 0);
    let root = output(
        "codebase_memory_search_graph",
        ROOT,
        DecisionAnchorLineageStageV1::Root,
    );
    let trace = output(
        "codebase_memory_trace_path",
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
    );
    let source_one = output(
        "codebase_memory_get_code_snippet",
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
    );
    let source_two = output(
        "codebase_memory_get_code_snippet",
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
    );
    state.on_tool_batch_finished(&[
        (
            "source-two",
            "codebase_memory_get_code_snippet",
            &source_two,
        ),
        ("trace", "codebase_memory_trace_path", &trace),
        ("root", "codebase_memory_search_graph", &root),
        (
            "source-one",
            "codebase_memory_get_code_snippet",
            &source_one,
        ),
    ]);
    assert!(
        state.blocks_mutation("write"),
        "the root must be consumed by a later model turn"
    );
}

#[test]
fn blocks_until_a_later_root_bound_trace_and_all_typed_sources_complete() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    state.on_tool_dispatched(&call("root", "codebase_memory_search_graph"), 0);
    finish(
        &mut state,
        "root",
        "codebase_memory_search_graph",
        ROOT,
        DecisionAnchorLineageStageV1::Root,
    );
    assert!(state.blocks_mutation("write"));
    assert_eq!(
        state.on_tool_dispatched(&call("blocked-write", "write"), 1),
        Some(ToolCallDenial::DecisionAnchorMutation),
        "mutation remains locally denied while current-root evidence is incomplete"
    );

    state.on_tool_dispatched(&call("trace", "codebase_memory_trace_path"), 1);
    finish(
        &mut state,
        "trace",
        "codebase_memory_trace_path",
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
    );
    state.on_tool_dispatched(
        &source_call("implementation", DecisionEvidenceKindV1::Implementation),
        2,
    );
    finish_with_evidence(
        &mut state,
        "implementation",
        ROOT,
        DecisionEvidenceKindV1::Implementation,
    );
    assert!(
        state.blocks_mutation("write"),
        "one typed source purpose is incomplete"
    );
    state.on_tool_dispatched(&source_call("caller", DecisionEvidenceKindV1::Caller), 3);
    finish_with_evidence(&mut state, "caller", ROOT, DecisionEvidenceKindV1::Caller);
    assert!(state.blocks_mutation("write"));
    state.on_tool_dispatched(&source_call("test", DecisionEvidenceKindV1::FocusedTest), 4);
    finish_with_evidence(
        &mut state,
        "test",
        ROOT,
        DecisionEvidenceKindV1::FocusedTest,
    );

    assert!(!state.blocks_mutation("write"));
}
