#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_denial_preserves_the_stable_missing_evidence_wire() {
        let details = GraphExplorationClosedV1::recoverable_without_actions(
            [GraphRecoveryEvidenceKindV1::Caller],
            4,
        )
        .expect("compact recovery denial");
        assert!(details.is_valid());
        assert_eq!(
            details.model_message(),
            "decision-evidence recovery required; missing evidence: [caller]; permitted action: targeted_current_root_graph_call; remaining allowance: 4"
        );
        assert_eq!(
            serde_json::to_value(details).unwrap(),
            serde_json::json!({
                "reason": "recoverable_incomplete_evidence",
                "missing_evidence": ["caller"],
                "permitted_action": "targeted_current_root_graph_call",
                "remaining_allowance": 4,
            })
        );
    }

    #[test]
    fn recoverable_message_is_an_exact_bounded_current_root_instruction() {
        const PRIVATE_SELECTOR: &str = "private::selector::must_not_escape";
        let details = GraphExplorationClosedV1::recoverable(
            [
                GraphRecoveryEvidenceKindV1::Caller,
                GraphRecoveryEvidenceKindV1::Trace,
            ],
            2,
        )
        .expect("actionable recovery guidance");

        let message = details.model_message();
        assert_eq!(
            message,
            "decision-evidence recovery required; missing evidence: [trace, caller]; permitted action: targeted_current_root_graph_call; remaining allowance: 2; compatible actions: [trace_path/function_name/trace/selector=implementation_evidence_result/relationship=calls/direction=inbound, get_code_snippet/qualified_name/caller/selector=caller_traversal_result]; use only these current-root actions; no retry, root switch, or mutation"
        );
        assert!(!message.contains(PRIVATE_SELECTOR));
        assert!(!message.contains("root_binding"));
        assert!(!message.contains("qualified_name="));
        assert!(details.is_valid());
    }

    #[test]
    fn no_compatible_action_fallback_is_closed_actionable_and_private() {
        const PRIVATE: [&str; 6] = [
            "private-root",
            "private::selector",
            "/private/path.rs",
            "provider payload",
            "source text",
            "target digest",
        ];
        let details = GraphExplorationClosedV1::conventional_fallback();
        assert!(details.is_valid());
        assert_eq!(
            details.model_message(),
            "graph exploration closed: no compatible provider-derived recovery action remains; permitted action: conventional_discovery; remaining allowance: 0"
        );
        let serialized = serde_json::to_string(&details).unwrap();
        assert_eq!(
            serialized,
            r#"{"reason":"no_compatible_recovery_action","missing_evidence":[],"permitted_action":"conventional_discovery","remaining_allowance":0}"#
        );
        assert!(PRIVATE.iter().all(|private| !serialized.contains(private)));
    }
}
