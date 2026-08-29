use jig_core::RequestView;

use super::*;

pub(super) fn reply(
    case: DecisionCase,
    result_count: usize,
    view: &RequestView,
    record: &impl Fn(DecisionStep),
    next_target: &impl Fn() -> String,
    recovery_selector: &impl Fn(&str) -> String,
    mutation_was_blocked: &impl Fn() -> bool,
) -> Option<Reply> {
    let reply = match (case, result_count) {
        (
            DecisionCase::ImplementationOnlyProviderFallback
            | DecisionCase::ImplementationFocusedProviderFallback,
            0,
        ) => {
            record(DecisionStep::Discovery);
            tool_reply(
                "partial-discovery",
                "codebase_memory_search_graph",
                serde_json::json!({"query": "implementation"}),
            )
        }
        (
            DecisionCase::ImplementationOnlyProviderFallback
            | DecisionCase::ImplementationFocusedProviderFallback,
            1,
        ) => {
            record(DecisionStep::Refinement);
            tool_reply(
                "partial-refinement",
                "codebase_memory_search_code",
                serde_json::json!({"pattern": next_target()}),
            )
        }
        (
            DecisionCase::ImplementationOnlyProviderFallback
            | DecisionCase::ImplementationFocusedProviderFallback,
            2,
        ) => {
            record(DecisionStep::ImplementationSource);
            tool_reply(
                "partial-implementation",
                "codebase_memory_get_code_snippet",
                serde_json::json!({
                    "qualified_name": next_target(),
                    "decision_evidence_kind": "implementation",
                }),
            )
        }
        (DecisionCase::ImplementationOnlyProviderFallback, 3) => {
            record(DecisionStep::ProviderFailure);
            tool_reply(
                "implementation-only-provider-failure",
                "codebase_memory_trace_path",
                serde_json::json!({
                    "function_name": recovery_selector("implementation_evidence_result"),
                    "mode": "calls",
                    "direction": "inbound",
                    "force_unavailable": true,
                }),
            )
        }
        (DecisionCase::ImplementationOnlyProviderFallback, 4) => {
            record(DecisionStep::MutationAttempt);
            tool_reply(
                "implementation-only-premature-mutation",
                "write",
                serde_json::json!({
                    "path": "demo/FALLBACK.md",
                    "content": "must remain blocked\n",
                }),
            )
        }
        (DecisionCase::ImplementationOnlyProviderFallback, 5) => {
            assert_fallback(view, mutation_was_blocked);
            assert!(view.messages.iter().any(|message| {
                message.role == "tool"
                    && message
                        .content
                        .contains("conventional fallback has been released")
            }));
            record(DecisionStep::MutationBlocked);
            record(DecisionStep::SourceRead);
            tool_reply(
                "implementation-only-fallback-read",
                "read",
                serde_json::json!({"path": "demo/FALLBACK.md"}),
            )
        }
        (DecisionCase::ImplementationOnlyProviderFallback, 6) => {
            record(DecisionStep::Mutation);
            tool_reply(
                "implementation-only-fallback-mutation",
                "write",
                serde_json::json!({
                    "path": "demo/FALLBACK.md",
                    "content": "independent conventional fallback completed\n",
                }),
            )
        }
        (DecisionCase::ImplementationOnlyProviderFallback, 7) => {
            record(DecisionStep::Complete);
            Reply::text(
                r#"{"summary":"Used an exact conventional fallback after provider failure."}"#,
            )
        }
        (DecisionCase::ImplementationFocusedProviderFallback, 3) => {
            record(DecisionStep::Trace);
            tool_reply(
                "implementation-focused-trace",
                "codebase_memory_trace_path",
                serde_json::json!({
                    "function_name": recovery_selector("implementation_evidence_result"),
                    "mode": "calls",
                    "direction": "inbound",
                }),
            )
        }
        (DecisionCase::ImplementationFocusedProviderFallback, 4) => {
            record(DecisionStep::CallerSource);
            tool_reply(
                "implementation-focused-caller",
                "codebase_memory_get_code_snippet",
                serde_json::json!({
                    "qualified_name": recovery_selector("caller_traversal_result"),
                    "decision_evidence_kind": "caller",
                }),
            )
        }
        (DecisionCase::ImplementationFocusedProviderFallback, 5) => {
            record(DecisionStep::SourceRead);
            tool_reply(
                "implementation-focused-graph-read",
                "read",
                serde_json::json!({"path": "demo/EVIDENCE.md"}),
            )
        }
        (DecisionCase::ImplementationFocusedProviderFallback, 6) => {
            record(DecisionStep::ProviderFailure);
            tool_reply(
                "implementation-focused-provider-failure",
                "codebase_memory_search_graph",
                serde_json::json!({
                    "query": "focused behavioral regression",
                    "force_unavailable": true,
                }),
            )
        }
        (DecisionCase::ImplementationFocusedProviderFallback, 7) => {
            record(DecisionStep::MutationAttempt);
            tool_reply(
                "implementation-focused-premature-mutation",
                "write",
                serde_json::json!({
                    "path": "demo/EVIDENCE.md",
                    "content": "must remain blocked\n",
                }),
            )
        }
        (DecisionCase::ImplementationFocusedProviderFallback, 8) => {
            assert_fallback(view, mutation_was_blocked);
            record(DecisionStep::MutationBlocked);
            record(DecisionStep::SourceRead);
            tool_reply(
                "implementation-focused-fallback-read",
                "read",
                serde_json::json!({"path": "demo/EVIDENCE.md"}),
            )
        }
        (DecisionCase::ImplementationFocusedProviderFallback, 9) => {
            record(DecisionStep::Mutation);
            tool_reply(
                "implementation-focused-fallback-mutation",
                "write",
                serde_json::json!({
                    "path": "demo/EVIDENCE.md",
                    "content": "implementation-focused fallback completed\n",
                }),
            )
        }
        (DecisionCase::ImplementationFocusedProviderFallback, 10) => {
            record(DecisionStep::Complete);
            Reply::text(r#"{"summary":"Revoked partial graph authority before fallback."}"#)
        }
        (DecisionCase::NoRetainedDecisionEvidence, 0) => {
            record(DecisionStep::Discovery);
            tool_reply(
                "oversized-targeted-result",
                "codebase_memory_search_graph",
                serde_json::json!({"query": "oversized-unretained"}),
            )
        }
        (DecisionCase::NoRetainedDecisionEvidence, 1) => {
            record(DecisionStep::MutationAttempt);
            tool_reply(
                "unretained-premature-mutation",
                "write",
                serde_json::json!({
                    "path": "demo/EVIDENCE.md",
                    "content": "must remain blocked\n",
                }),
            )
        }
        (DecisionCase::NoRetainedDecisionEvidence, 2) => {
            assert!(mutation_was_blocked());
            record(DecisionStep::MutationBlocked);
            record(DecisionStep::Complete);
            Reply::text(r#"{"summary":"Stopped after unretained graph output."}"#)
        }
        _ => return None,
    };
    Some(reply)
}

fn assert_fallback(view: &RequestView, mutation_was_blocked: &impl Fn() -> bool) {
    assert!(mutation_was_blocked());
    assert!(view.messages.iter().any(|message| {
        message.role == "user" && message.content.contains("minimal conventional fallback")
    }));
}
