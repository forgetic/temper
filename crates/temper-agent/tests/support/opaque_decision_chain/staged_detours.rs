use jig_core::RequestView;

use super::*;

pub(super) fn reply(
    case: DecisionCase,
    result_count: usize,
    view: &RequestView,
    record: &impl Fn(DecisionStep),
    next_target: &impl Fn() -> String,
    focused_test_target: &impl Fn() -> String,
    source_target: &impl Fn(&str) -> String,
    recovery_selector: &impl Fn(&str) -> String,
) -> Option<Reply> {
    if case != DecisionCase::StagedDetourRecovery {
        return None;
    }
    let reply = match result_count {
        0 => {
            record(DecisionStep::Discovery);
            tool_reply(
                "discover-staged-implementation",
                "codebase_memory_search_graph",
                serde_json::json!({"query": "implementation"}),
            )
        }
        1 => {
            assert_guidance(
                view,
                &[
                    "result=active_root_progress",
                    "accepted evidence=[root]",
                    "active-root missing evidence=[trace, implementation, caller, focused_test]",
                ],
            );
            record(DecisionStep::Refinement);
            tool_reply(
                "refine-staged-implementation",
                "codebase_memory_search_code",
                serde_json::json!({"pattern": next_target()}),
            )
        }
        2 => {
            record(DecisionStep::ImplementationSource);
            tool_reply(
                "read-staged-implementation",
                "codebase_memory_get_code_snippet",
                serde_json::json!({
                    "qualified_name": next_target(),
                    "decision_evidence_kind": "implementation",
                }),
            )
        }
        3 => {
            assert_guidance(
                view,
                &[
                    "accepted evidence=[implementation]",
                    "active-root missing evidence=[trace, caller, focused_test]",
                ],
            );
            record(DecisionStep::Trace);
            tool_reply(
                "trace-staged-implementation",
                "codebase_memory_trace_path",
                serde_json::json!({
                    "function_name": recovery_selector("implementation_evidence_result"),
                    "direction": "inbound",
                }),
            )
        }
        4 => {
            record(DecisionStep::CallerSourceDetour);
            tool_reply(
                "caller-shaped-but-ineligible",
                "codebase_memory_get_code_snippet",
                serde_json::json!({
                    "qualified_name": source_target("implementation_source"),
                    "decision_evidence_kind": "caller",
                }),
            )
        }
        5 => {
            assert!(view.messages.iter().any(|message| {
                message.role == "tool"
                    && message
                        .content
                        .contains("decision-evidence recovery required")
            }));
            assert_guidance(
                view,
                &[
                    "result=non_progress",
                    "required next stage=[get_code_snippet/qualified_name/caller/selector=caller_traversal_result]",
                ],
            );
            record(DecisionStep::CallerSource);
            tool_reply(
                "read-exact-traversal-caller",
                "codebase_memory_get_code_snippet",
                serde_json::json!({
                    "qualified_name": recovery_selector("caller_traversal_result"),
                    "decision_evidence_kind": "caller",
                }),
            )
        }
        count @ 6..=7 => {
            assert_guidance(
                view,
                &[
                    "active-root missing evidence=[focused_test]",
                    "required next stage=[get_code_snippet/qualified_name/focused_test/selector=focused_test_result]",
                ],
            );
            record(DecisionStep::FocusedTestDetour);
            tool_reply(
                &format!("unsupported-focused-traversal-{count}"),
                "codebase_memory_trace_path",
                serde_json::json!({
                    "function_name": recovery_selector("caller_evidence_result"),
                    "mode": "calls",
                    "direction": "inbound",
                    "include_tests": true,
                }),
            )
        }
        8 => {
            let local_detours = view
                .messages
                .iter()
                .filter(|message| {
                    message.role == "tool"
                        && message
                            .content
                            .contains("decision-evidence recovery required")
                })
                .count();
            assert_eq!(local_detours, 3, "all three detours stay local");
            record(DecisionStep::BehavioralTestSource);
            tool_reply(
                "read-exact-semantic-test",
                "codebase_memory_get_code_snippet",
                serde_json::json!({
                    "qualified_name": focused_test_target(),
                    "decision_evidence_kind": "focused_test",
                }),
            )
        }
        9 => {
            record(DecisionStep::SourceRead);
            tool_reply(
                "read-detour-recovery-target",
                "read",
                serde_json::json!({"path": "demo/EVIDENCE.md"}),
            )
        }
        10 => {
            record(DecisionStep::Mutation);
            tool_reply(
                "mutate-after-detour-recovery",
                "write",
                serde_json::json!({
                    "path": "demo/EVIDENCE.md",
                    "content": "staged detours recovered\n",
                }),
            )
        }
        11 => {
            record(DecisionStep::Complete);
            Reply::text(r#"{"summary":"Recovered the exact staged graph chain."}"#)
        }
        turn => panic!("unexpected staged-detour model turn {turn}"),
    };
    Some(reply)
}
