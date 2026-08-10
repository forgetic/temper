//! Deterministic decision-anchor policy regressions.

mod tests {
    use super::super::super::decision_anchor::*;
    use crate::machine::SAFE_GRAPH_CORRELATION_DETAIL_KEY;
    use std::collections::BTreeMap;
    use temper_protocol_activity::{
        GraphCorrelationTargetKindV1, GraphCorrelationToolV1, GraphCorrelationV1,
    };
    use tongs::{
        model::ToolCall,
        tools::{ToolEffects, ToolOutput},
    };

    fn effects() -> BTreeMap<String, ToolEffects> {
        [
            ("codebase_memory_search_graph", ToolEffects::read()),
            ("codebase_memory_search_code", ToolEffects::read()),
            ("codebase_memory_trace_path", ToolEffects::read()),
            ("codebase_memory_get_code_snippet", ToolEffects::read()),
            ("write", ToolEffects::write()),
        ]
        .into_iter()
        .map(|(name, effect)| (name.to_string(), effect))
        .collect()
    }

    fn call(id: &str, name: &str, target: &str) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments: serde_json::json!({"target": target}),
        }
    }

    fn output(name: &str, target: &str) -> ToolOutput {
        let (tool, kind) = match name {
            "codebase_memory_search_graph" => (
                GraphCorrelationToolV1::SearchGraph,
                GraphCorrelationTargetKindV1::GraphQuery,
            ),
            "codebase_memory_search_code" => (
                GraphCorrelationToolV1::SearchCode,
                GraphCorrelationTargetKindV1::Pattern,
            ),
            "codebase_memory_trace_path" => (
                GraphCorrelationToolV1::TracePath,
                GraphCorrelationTargetKindV1::FunctionName,
            ),
            "codebase_memory_get_code_snippet" => (
                GraphCorrelationToolV1::GetCodeSnippet,
                GraphCorrelationTargetKindV1::QualifiedName,
            ),
            _ => panic!("unsupported test tool"),
        };
        ToolOutput {
            content: Vec::new(),
            details: Some(serde_json::json!({
                SAFE_GRAPH_CORRELATION_DETAIL_KEY: GraphCorrelationV1::new(tool, kind, "request").unwrap(),
                SAFE_DECISION_ANCHOR_DETAIL_KEY: DecisionAnchorEvidenceV1::new(
                    [GraphCorrelationV1::target_digest(target).unwrap()]
                ),
            })),
            is_error: false,
        }
    }

    fn finish(state: &mut DecisionAnchorState, id: &str, name: &str, target: &str) {
        state.on_tool_finished(id, name, &output(name, target));
    }

    #[test]
    fn blocks_until_a_later_result_derived_trace_and_two_source_reads_complete() {
        let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
        state.on_tool_dispatched(&call("root", "codebase_memory_search_graph", "start"), 0);
        finish(&mut state, "root", "codebase_memory_search_graph", "refine");
        assert!(state.blocks_mutation("write"));

        state.on_tool_dispatched(&call("refine", "codebase_memory_search_code", "refine"), 1);
        finish(&mut state, "refine", "codebase_memory_search_code", "trace");
        state.on_tool_dispatched(&call("trace", "codebase_memory_trace_path", "trace"), 2);
        finish(
            &mut state,
            "trace",
            "codebase_memory_trace_path",
            "implementation",
        );
        state.on_tool_dispatched(
            &call(
                "source",
                "codebase_memory_get_code_snippet",
                "implementation",
            ),
            3,
        );
        finish(
            &mut state,
            "source",
            "codebase_memory_get_code_snippet",
            "behavior",
        );
        assert!(
            state.blocks_mutation("write"),
            "one source read is incomplete"
        );
        state.on_tool_dispatched(
            &call("test", "codebase_memory_get_code_snippet", "behavior"),
            4,
        );
        finish(
            &mut state,
            "test",
            "codebase_memory_get_code_snippet",
            "done",
        );

        assert!(!state.blocks_mutation("write"));
    }

    #[test]
    fn unrelated_or_producer_turn_calls_cannot_replace_a_pending_anchor() {
        let mut unrelated = DecisionAnchorState::from_effects(&effects()).unwrap();
        unrelated.on_tool_dispatched(&call("root", "codebase_memory_search_graph", "start"), 0);
        finish(
            &mut unrelated,
            "root",
            "codebase_memory_search_graph",
            "expected",
        );
        unrelated.on_tool_dispatched(
            &call("wrong", "codebase_memory_search_code", "unrelated"),
            1,
        );
        finish(
            &mut unrelated,
            "wrong",
            "codebase_memory_search_code",
            "replacement",
        );
        assert!(unrelated.blocks_mutation("write"));

        let mut producer_turn = DecisionAnchorState::from_effects(&effects()).unwrap();
        producer_turn.on_tool_dispatched(&call("root", "codebase_memory_search_graph", "start"), 0);
        producer_turn
            .on_tool_dispatched(&call("same", "codebase_memory_search_code", "expected"), 0);
        finish(
            &mut producer_turn,
            "root",
            "codebase_memory_search_graph",
            "expected",
        );
        finish(
            &mut producer_turn,
            "same",
            "codebase_memory_search_code",
            "trace",
        );
        assert!(producer_turn.blocks_mutation("write"));
    }

    #[test]
    fn conventional_reads_do_not_consume_an_anchor_and_empty_or_failed_results_do_not_activate_one()
    {
        let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
        state.on_tool_dispatched(&call("root", "codebase_memory_search_graph", "start"), 0);
        finish(
            &mut state,
            "root",
            "codebase_memory_search_graph",
            "expected",
        );
        assert!(state.blocks_mutation("write"));

        let mut no_anchor = DecisionAnchorState::from_effects(&effects()).unwrap();
        let mut failed = output("codebase_memory_search_graph", "expected");
        failed.is_error = true;
        no_anchor.on_tool_dispatched(&call("failed", "codebase_memory_search_graph", "start"), 0);
        no_anchor.on_tool_finished("failed", "codebase_memory_search_graph", &failed);
        assert!(!no_anchor.blocks_mutation("write"));
    }

    #[test]
    fn evidence_is_canonical_and_contains_no_raw_target() {
        let raw = "Authorization: Bearer decision-anchor-secret";
        let evidence = DecisionAnchorEvidenceV1::new([
            GraphCorrelationV1::target_digest(raw).unwrap(),
            "not-a-digest".to_string(),
        ]);
        assert!(evidence.is_valid());
        let serialized = serde_json::to_string(&evidence).unwrap();
        assert!(!serialized.contains(raw));
        assert!(!serialized.contains("not-a-digest"));
    }
}
