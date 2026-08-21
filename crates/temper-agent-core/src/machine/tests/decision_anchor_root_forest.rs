// Multi-root decision-anchor regressions.

use super::*;

const THIRD_ROOT: &str = "00000000-0000-4000-8000-000000000003";

#[test]
fn three_discovery_roots_jointly_cover_next_turn_trace_and_sources() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    for id in ["affinity-root", "retry-root", "caller-root"] {
        state.on_tool_dispatched(&call(id, "codebase_memory_search_graph"), 0);
    }
    let affinity_root = output_with_kinds(
        "codebase_memory_search_graph",
        ROOT,
        DecisionAnchorLineageStageV1::Root,
        &[
            DecisionAnchorTargetKindV1::FunctionName,
            DecisionAnchorTargetKindV1::QualifiedName,
        ],
    );
    let retry_root = output_with_kinds(
        "codebase_memory_search_graph",
        OTHER_ROOT,
        DecisionAnchorLineageStageV1::Root,
        &[DecisionAnchorTargetKindV1::QualifiedName],
    );
    let caller_root = output_with_kinds(
        "codebase_memory_search_graph",
        THIRD_ROOT,
        DecisionAnchorLineageStageV1::Root,
        &[DecisionAnchorTargetKindV1::QualifiedName],
    );
    assert_eq!(
        state.on_tool_batch_finished(&[
            ("caller-root", "codebase_memory_search_graph", &caller_root),
            (
                "affinity-root",
                "codebase_memory_search_graph",
                &affinity_root,
            ),
            ("retry-root", "codebase_memory_search_graph", &retry_root),
        ]),
        DecisionAnchorTransition::Unchanged,
    );
    assert!(state.blocks_mutation("write"));

    state.on_tool_dispatched(&call("affinity-trace", "codebase_memory_trace_path"), 1);
    state.on_tool_dispatched(
        &source_call("affinity-source", DecisionEvidenceKindV1::Implementation),
        1,
    );
    state.on_tool_dispatched(
        &source_call("retry-source", DecisionEvidenceKindV1::FocusedTest),
        1,
    );
    state.on_tool_dispatched(
        &source_call("caller-source", DecisionEvidenceKindV1::Caller),
        1,
    );
    let affinity_trace = output(
        "codebase_memory_trace_path",
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
    );
    let affinity_source = output_with_evidence(
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
        DecisionEvidenceKindV1::Implementation,
    );
    let retry_source = output_with_evidence(
        OTHER_ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
        DecisionEvidenceKindV1::FocusedTest,
    );
    let caller_source = output_with_evidence(
        THIRD_ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
        DecisionEvidenceKindV1::Caller,
    );
    assert_eq!(
        state.on_tool_batch_finished(&[
            (
                "retry-source",
                "codebase_memory_get_code_snippet",
                &retry_source,
            ),
            (
                "affinity-source",
                "codebase_memory_get_code_snippet",
                &affinity_source,
            ),
            (
                "caller-source",
                "codebase_memory_get_code_snippet",
                &caller_source,
            ),
            (
                "affinity-trace",
                "codebase_memory_trace_path",
                &affinity_trace,
            ),
        ]),
        DecisionAnchorTransition::Converged,
    );
    assert!(
        !state.blocks_mutation("write"),
        "each descendant remains bound to its own root while the forest jointly covers evidence"
    );
}

#[test]
fn later_batch_roots_join_the_forest_but_same_batch_descendants_stay_ineligible() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    state.on_tool_dispatched(&call("initial", "codebase_memory_search_code"), 0);
    finish(
        &mut state,
        "initial",
        "codebase_memory_search_code",
        ROOT,
        DecisionAnchorLineageStageV1::Root,
    );

    state.on_tool_dispatched(
        &call("new-root", "codebase_memory_get_code_snippet"),
        1,
    );
    state.on_tool_dispatched(&call("same-batch-trace", "codebase_memory_trace_path"), 1);
    let new_root = output(
        "codebase_memory_get_code_snippet",
        OTHER_ROOT,
        DecisionAnchorLineageStageV1::Root,
    );
    let same_batch_trace = output(
        "codebase_memory_trace_path",
        OTHER_ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
    );
    assert_eq!(
        state.on_tool_batch_finished(&[
            (
                "new-root",
                "codebase_memory_get_code_snippet",
                &new_root,
            ),
            (
                "same-batch-trace",
                "codebase_memory_trace_path",
                &same_batch_trace,
            ),
        ]),
        DecisionAnchorTransition::Unchanged,
    );
    assert!(
        state.blocks_mutation("write"),
        "a root discovered in a later batch is retained but its sibling cannot consume it"
    );
}

#[test]
fn later_independent_root_expansion_is_bounded() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    state.on_tool_dispatched(&call("initial", "codebase_memory_search_graph"), 0);
    finish(
        &mut state,
        "initial",
        "codebase_memory_search_graph",
        ROOT,
        DecisionAnchorLineageStageV1::Root,
    );

    let later_roots = [
        OTHER_ROOT,
        THIRD_ROOT,
        "00000000-0000-4000-8000-000000000004",
        "00000000-0000-4000-8000-000000000005",
        "00000000-0000-4000-8000-000000000006",
    ];
    for (index, root) in later_roots.into_iter().enumerate() {
        let id = format!("later-root-{index}");
        state.on_tool_dispatched(
            &call(&id, "codebase_memory_search_graph"),
            index.saturating_add(1),
        );
        let transition = state.on_tool_finished(
            &id,
            "codebase_memory_search_graph",
            &output(
                "codebase_memory_search_graph",
                root,
                DecisionAnchorLineageStageV1::Root,
            ),
        );
        if index < MAX_LATER_DECISION_ANCHOR_ROOTS {
            assert_eq!(transition, DecisionAnchorTransition::Unchanged);
        } else {
            assert_eq!(transition, DecisionAnchorTransition::GapRecoveryNeeded);
        }
    }
    assert_eq!(
        state.on_tool_dispatched(&call("after-limit", "codebase_memory_search_graph"), 7),
        recovery_graph_denial(all_missing(), 4)
    );
    assert!(state.blocks_mutation("write"));
}

#[test]
fn independent_trace_root_is_evidence_for_later_forest_sources() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    for (id, tool) in [
        ("trace-root", "codebase_memory_trace_path"),
        ("implementation-root", "codebase_memory_get_code_snippet"),
    ] {
        state.on_tool_dispatched(&call(id, tool), 0);
    }
    let trace_root = output(
        "codebase_memory_trace_path",
        ROOT,
        DecisionAnchorLineageStageV1::Root,
    );
    let implementation_root = output(
        "codebase_memory_get_code_snippet",
        OTHER_ROOT,
        DecisionAnchorLineageStageV1::Root,
    );
    state.on_tool_batch_finished(&[
        ("trace-root", "codebase_memory_trace_path", &trace_root),
        (
            "implementation-root",
            "codebase_memory_get_code_snippet",
            &implementation_root,
        ),
    ]);

    state.on_tool_dispatched(
        &source_call(
            "implementation-source",
            DecisionEvidenceKindV1::Implementation,
        ),
        1,
    );
    state.on_tool_dispatched(
        &source_call("caller-source", DecisionEvidenceKindV1::Caller),
        1,
    );
    state.on_tool_dispatched(
        &source_call("test-source", DecisionEvidenceKindV1::FocusedTest),
        1,
    );
    let implementation_source = output_with_evidence(
        OTHER_ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
        DecisionEvidenceKindV1::Implementation,
    );
    let caller_source = output_with_evidence(
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
        DecisionEvidenceKindV1::Caller,
    );
    let test_source = output_with_evidence(
        OTHER_ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
        DecisionEvidenceKindV1::FocusedTest,
    );
    state.on_tool_batch_finished(&[
        (
            "implementation-source",
            "codebase_memory_get_code_snippet",
            &implementation_source,
        ),
        (
            "caller-source",
            "codebase_memory_get_code_snippet",
            &caller_source,
        ),
        (
            "test-source",
            "codebase_memory_get_code_snippet",
            &test_source,
        ),
    ]);
    assert!(!state.blocks_mutation("write"));
}
