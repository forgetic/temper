// Decision-anchor recovery and malformed-result regressions.

use super::*;

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
