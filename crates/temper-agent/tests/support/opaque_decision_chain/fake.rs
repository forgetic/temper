//! Scripted model for opaque decision-chain integration cases.

use super::*;

pub(super) fn decision_chain_fake(
    case: DecisionCase,
    observed_steps: Arc<Mutex<Vec<DecisionStep>>>,
) -> FakeLlm {
    FakeLlm::start(Script::rule(move |view| {
        let provider_values = |field: &str| {
            view.messages
                .iter()
                .filter_map(|message| {
                    if message.role != "tool" {
                        return None;
                    }
                    let pointer = format!("/results/0/{field}");
                    provider_result(&message.content)?
                        .pointer(&pointer)
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                })
                .collect::<Vec<_>>()
        };
        let result_count = || {
            view.messages
                .iter()
                .filter(|message| {
                    message.role == "tool"
                        && provider_result(&message.content)
                            .is_some_and(|value| value.get("results").is_some())
                })
                .count()
        };
        let next_target = || {
            provider_values("next")
                .pop()
                .expect("the prior provider-shaped result selected a next target")
        };
        let recovery_selector = |label: &str| {
            let marker = format!("{label}=temper-recovery-selector:");
            view.messages
                .iter()
                .rev()
                .find_map(|message| {
                    let start = message.content.find(&marker)? + label.len() + 1;
                    let value = &message.content[start..];
                    let end = value
                        .find([',', ' ', ']', '.'])
                        .unwrap_or(value.len());
                    Some(value[..end].to_string())
                })
                .unwrap_or_else(|| panic!("missing provider-derived {label} reference"))
        };
        let record = |step| observed_steps.lock().expect("decision steps lock").push(step);
        let mutation_was_blocked = || {
            view.messages.iter().any(|message| {
                message.role == "tool"
                    && message
                        .content
                        .contains("workspace mutation blocked: use the ordinary read tool")
            })
        };

        match (case, view.prior_tool_results) {
            (DecisionCase::Consumed, 0) => {
                record(DecisionStep::Discovery);
                tool_reply(
                    "discover-implementation",
                    "codebase_memory_search_graph",
                    serde_json::json!({"query": "implementation"}),
                )
            }
            (DecisionCase::Consumed, 1) => {
                assert_guidance(
                    view,
                    &[
                        "result=active_root_progress",
                        "accepted evidence=[root]",
                        "active-root missing evidence=[trace, implementation, caller, focused_test]",
                    ],
                );
                assert!(
                    !provider_values("current_root").is_empty(),
                    "refinement requires a consumed current-root implementation result"
                );
                record(DecisionStep::Refinement);
                tool_reply(
                    "refine-implementation",
                    "codebase_memory_search_code",
                    serde_json::json!({"pattern": next_target()}),
                )
            }
            (DecisionCase::Consumed, 2) => {
                record(DecisionStep::ImplementationSource);
                tool_reply(
                    "read-implementation",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({
                        "qualified_name": next_target(),
                        "decision_evidence_kind": "implementation",
                    }),
                )
            }
            (DecisionCase::Consumed, 3) => {
                assert_guidance(
                    view,
                    &[
                        "accepted evidence=[implementation]",
                        "active-root missing evidence=[trace, caller, focused_test]",
                    ],
                );
                record(DecisionStep::Trace);
                tool_reply(
                    "trace-selected-implementation",
                    "codebase_memory_trace_path",
                    serde_json::json!({
                        "function_name": recovery_selector("implementation_evidence_result"),
                        "direction": "inbound",
                    }),
                )
            }
            (DecisionCase::Consumed, 4) => {
                assert_guidance(
                    view,
                    &[
                        "accepted evidence=[trace]",
                        "active-root missing evidence=[caller, focused_test]",
                    ],
                );
                record(DecisionStep::CallerSource);
                tool_reply(
                    "read-traversal-caller",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({
                        "qualified_name": recovery_selector("caller_traversal_result"),
                        "decision_evidence_kind": "caller",
                    }),
                )
            }
            (DecisionCase::Consumed, 5) => {
                assert_guidance(
                    view,
                    &[
                        "accepted evidence=[caller]",
                        "active-root missing evidence=[focused_test]",
                    ],
                );
                record(DecisionStep::FocusedTestTraversal);
                tool_reply(
                    "trace-caller-tests",
                    "codebase_memory_trace_path",
                    serde_json::json!({
                        "function_name": recovery_selector("caller_evidence_result"),
                        "mode": "calls",
                        "direction": "inbound",
                        "include_tests": true,
                    }),
                )
            }
            (DecisionCase::Consumed, 6) => {
                assert_guidance(
                    view,
                    &[
                        "accepted evidence=[focused_test_route]",
                        "active-root missing evidence=[focused_test]",
                    ],
                );
                record(DecisionStep::BehavioralTestSource);
                tool_reply(
                    "read-behavioral-test",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({
                        "qualified_name": recovery_selector("focused_test_result"),
                        "decision_evidence_kind": "focused_test",
                    }),
                )
            }
            (DecisionCase::Consumed, 7) => {
                assert_guidance(
                    view,
                    &[
                        "accepted evidence=[focused_test]",
                        "active-root missing evidence=[]",
                        "read/workspace_target/exact_post_source",
                        "matching minimal mutation",
                    ],
                );
                assert!(
                    !provider_values("current_root").is_empty()
                        && provider_values("caller_model").len() == 1
                        && provider_values("implementation_source").len() == 1
                        && provider_values("behavioral_test").len() == 1,
                    "mutation requires consumed current-root, caller/model, and focused behavioral-test evidence"
                );
                tool_reply(
                    "read-exact-target-after-evidence",
                    "read",
                    serde_json::json!({"path": "demo/EVIDENCE.md"}),
                )
            }
            (DecisionCase::Consumed, 8) => {
                record(DecisionStep::Mutation);
                tool_reply(
                    "mutate-after-evidence",
                    "write",
                    serde_json::json!({
                        "path": "demo/EVIDENCE.md",
                        "content": "verified evidence\n"
                    }),
                )
            }
            (DecisionCase::Consumed, 9) => {
                record(DecisionStep::Complete);
                Reply::text(r#"{"summary":"Mutated after consumed result-derived evidence."}"#)
            }
            (DecisionCase::UnrelatedLaterTarget, 0) => {
                record(DecisionStep::Discovery);
                tool_reply(
                    "discover-implementation",
                    "codebase_memory_search_graph",
                    serde_json::json!({"query": "implementation"}),
                )
            }
            (DecisionCase::UnrelatedLaterTarget, 1) => {
                assert!(
                    !provider_values("current_root").is_empty(),
                    "the unrelated-target case starts after a successful producer"
                );
                record(DecisionStep::UnrelatedLaterTarget);
                tool_reply(
                    "refine-unrelated-target",
                    "codebase_memory_search_code",
                    serde_json::json!({"pattern": "not-derived"}),
                )
            }
            (DecisionCase::UnrelatedLaterTarget, 2) => {
                assert_eq!(
                    result_count(), 2,
                    "the unrelated target still receives a provider-shaped successful result"
                );
                record(DecisionStep::MutationAttempt);
                tool_reply(
                    "blocked-unrelated-mutation",
                    "write",
                    serde_json::json!({
                        "path": "demo/EVIDENCE.md",
                        "content": "must not be written\n"
                    }),
                )
            }
            (DecisionCase::UnrelatedLaterTarget, 3) => {
                assert!(mutation_was_blocked(), "the core must deny the unrelated mutation");
                record(DecisionStep::MutationBlocked);
                record(DecisionStep::Complete);
                Reply::text(r#"{"summary":"Stopped after a blocked unconsumed decision-chain mutation."}"#)
            }
            (DecisionCase::ProducerTurnDependents, 0) => {
                record(DecisionStep::ProducerTurnDependents);
                tool_replies(&[
                    (
                        "discover-implementation",
                        "codebase_memory_search_graph",
                        serde_json::json!({"query": "implementation"}),
                    ),
                    (
                        "refine-without-result",
                        "codebase_memory_search_code",
                        serde_json::json!({"pattern": "not-derived"}),
                    ),
                    (
                        "trace-without-result",
                        "codebase_memory_trace_path",
                        serde_json::json!({"function_name": "not-derived"}),
                    ),
                    (
                        "read-without-result",
                        "codebase_memory_get_code_snippet",
                        serde_json::json!({"qualified_name": "not-derived"}),
                    ),
                ])
            }
            (DecisionCase::ProducerTurnDependents, 4) => {
                assert_eq!(
                    result_count(), 4,
                    "producer-turn refinement, trace, and source reads all returned successfully"
                );
                record(DecisionStep::MutationAttempt);
                tool_reply(
                    "blocked-producer-mutation",
                    "write",
                    serde_json::json!({
                        "path": "demo/EVIDENCE.md",
                        "content": "must not be written\n"
                    }),
                )
            }
            (DecisionCase::ProducerTurnDependents, 5) => {
                assert!(mutation_was_blocked(), "the core must deny the producer-turn mutation");
                record(DecisionStep::MutationBlocked);
                record(DecisionStep::Complete);
                Reply::text(r#"{"summary":"Stopped after a blocked producer-turn mutation."}"#)
            }
            (DecisionCase::ConventionalReadSubstitution, 0) => {
                record(DecisionStep::Discovery);
                tool_reply(
                    "discover-implementation",
                    "codebase_memory_search_graph",
                    serde_json::json!({"query": "implementation"}),
                )
            }
            (DecisionCase::ConventionalReadSubstitution, 1) => {
                record(DecisionStep::ImplementationSource);
                tool_reply("conventional-read", "read", serde_json::json!({"path": "demo/README.md"}))
            }
            (DecisionCase::ConventionalReadSubstitution, 2) => {
                record(DecisionStep::MutationAttempt);
                tool_reply(
                    "blocked-conventional-mutation",
                    "write",
                    serde_json::json!({
                        "path": "demo/EVIDENCE.md",
                        "content": "must not be written\n"
                    }),
                )
            }
            (DecisionCase::ConventionalReadSubstitution, 3) => {
                assert!(mutation_was_blocked(), "conventional reads must not consume the anchor");
                record(DecisionStep::MutationBlocked);
                record(DecisionStep::Complete);
                Reply::text(r#"{"summary":"Stopped after a blocked conventional-read substitution."}"#)
            }
            (DecisionCase::IncompleteSourceEvidence, 0) => {
                record(DecisionStep::Discovery);
                tool_reply(
                    "discover-implementation",
                    "codebase_memory_search_graph",
                    serde_json::json!({"query": "implementation"}),
                )
            }
            (DecisionCase::IncompleteSourceEvidence, 1) => {
                record(DecisionStep::Refinement);
                tool_reply(
                    "refine-implementation",
                    "codebase_memory_search_code",
                    serde_json::json!({"pattern": next_target()}),
                )
            }
            (DecisionCase::IncompleteSourceEvidence, 2) => {
                record(DecisionStep::ImplementationSource);
                tool_reply(
                    "read-implementation",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({
                        "qualified_name": next_target(),
                        "decision_evidence_kind": "implementation",
                    }),
                )
            }
            (DecisionCase::IncompleteSourceEvidence, 3) => {
                record(DecisionStep::Trace);
                tool_reply(
                    "trace-caller-or-model",
                    "codebase_memory_trace_path",
                    serde_json::json!({
                        "function_name": next_target(),
                        "direction": "inbound",
                    }),
                )
            }
            (DecisionCase::IncompleteSourceEvidence, 4) => {
                record(DecisionStep::MutationAttempt);
                tool_reply(
                    "blocked-incomplete-mutation",
                    "write",
                    serde_json::json!({
                        "path": "demo/EVIDENCE.md",
                        "content": "must not be written\n"
                    }),
                )
            }
            (DecisionCase::IncompleteSourceEvidence, 5) => {
                assert!(mutation_was_blocked(), "one source read must not satisfy the evidence gate");
                record(DecisionStep::MutationBlocked);
                record(DecisionStep::Complete);
                Reply::text(r#"{"summary":"Stopped after a blocked incomplete-evidence mutation."}"#)
            }
            (DecisionCase::UnavailableAfterRoot, 0) => {
                record(DecisionStep::Discovery);
                tool_reply(
                    "discover-implementation",
                    "codebase_memory_search_graph",
                    serde_json::json!({"query": "implementation"}),
                )
            }
            (DecisionCase::UnavailableAfterRoot, 1) => {
                record(DecisionStep::Refinement);
                tool_reply(
                    "unavailable-refinement",
                    "codebase_memory_search_code",
                    serde_json::json!({
                        "pattern": next_target(),
                        "force_unavailable": true,
                    }),
                )
            }
            (DecisionCase::UnavailableAfterRoot, 2) => {
                assert!(
                    view.messages.iter().any(|message| {
                        message.role == "tool"
                            && message
                                .content
                                .contains("do not retry codebase-memory immediately")
                    }),
                    "the trusted unavailable result must provide bounded fallback guidance"
                );
                record(DecisionStep::UnavailableFallback);
                tool_reply(
                    "conventional-fallback-read",
                    "read",
                    serde_json::json!({"path": "demo/README.md"}),
                )
            }
            (DecisionCase::UnavailableAfterRoot, 3) => {
                record(DecisionStep::Mutation);
                tool_reply(
                    "mutate-after-unavailable-fallback",
                    "write",
                    serde_json::json!({
                        "path": "demo/EVIDENCE.md",
                        "content": "conventional fallback after unavailable provider\n",
                    }),
                )
            }
            (DecisionCase::UnavailableAfterRoot, 4) => {
                record(DecisionStep::Complete);
                Reply::text(r#"{"summary":"Used conventional discovery after an unavailable provider."}"#)
            }
            (DecisionCase::UnconsumableRecoveryExhausted, 0) => {
                record(DecisionStep::Discovery);
                tool_reply(
                    "discover-unconsumable",
                    "codebase_memory_search_graph",
                    serde_json::json!({"query": "unconsumable"}),
                )
            }
            (DecisionCase::UnconsumableRecoveryExhausted, turn @ 1..=2) => {
                let corrections = view
                    .messages
                    .iter()
                    .filter(|message| {
                        message.role == "user"
                            && message.content.contains("decision-anchor recovery required")
                    })
                    .collect::<Vec<_>>();
                assert!(
                    !corrections.is_empty(),
                    "the native agent must inject a generic recovery state"
                );
                assert!(
                    corrections
                        .iter()
                        .all(|message| message.content.contains("compatible current-root descendant")),
                    "recovery guidance must retain the bounded current-root policy"
                );
                assert!(
                    corrections
                        .iter()
                        .all(|message| !message.content.contains("PRIVATE-UNCONSUMABLE-SENTINEL")),
                    "recovery guidance must not retain provider values"
                );
                record(DecisionStep::Recovery);
                tool_reply(
                    &format!("recover-unconsumable-{turn}"),
                    "codebase_memory_search_graph",
                    serde_json::json!({"query": "unconsumable"}),
                )
            }
            (DecisionCase::AllRootsNonViableFallback, 0) => {
                record(DecisionStep::Discovery);
                tool_replies(&[
                    (
                        "nonviable-root-one",
                        "codebase_memory_search_code",
                        serde_json::json!({"pattern": "nonviable-one"}),
                    ),
                    (
                        "nonviable-root-two",
                        "codebase_memory_search_code",
                        serde_json::json!({"pattern": "nonviable-two"}),
                    ),
                    (
                        "nonviable-root-three",
                        "codebase_memory_search_code",
                        serde_json::json!({"pattern": "nonviable-three"}),
                    ),
                ])
            }
            (DecisionCase::AllRootsNonViableFallback, count @ 3..=4) => {
                record(DecisionStep::Recovery);
                tool_reply(
                    &format!("bounded-non-progress-{count}"),
                    "codebase_memory_get_architecture",
                    serde_json::json!({}),
                )
            }
            (DecisionCase::AllRootsNonViableFallback, count @ 5 | count @ 7 | count @ 9) => {
                let root_index = (count - 5) / 2;
                let target = provider_values("qualified_name")
                    .get(root_index)
                    .cloned()
                    .expect("the retained root exposes its trace selector");
                record(DecisionStep::Trace);
                tool_reply(
                    &format!("nonviable-trace-{root_index}"),
                    "codebase_memory_trace_path",
                    serde_json::json!({
                        "function_name": target,
                        "mode": "calls",
                        "direction": "inbound",
                    }),
                )
            }
            (DecisionCase::AllRootsNonViableFallback, count @ 6 | count @ 8 | count @ 10) => {
                let root_index = (count - 6) / 2;
                let target = provider_values("qualified_name")
                    .get(root_index)
                    .cloned()
                    .expect("the retained root exposes its implementation selector");
                record(DecisionStep::ImplementationSource);
                tool_reply(
                    &format!("nonviable-implementation-{root_index}"),
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({
                        "qualified_name": target,
                        "decision_evidence_kind": "implementation",
                    }),
                )
            }
            (DecisionCase::AllRootsNonViableFallback, 11) => {
                let guidance = view
                    .messages
                    .iter()
                    .filter(|message| {
                        message.role == "user"
                            && message.content.contains("minimal conventional fallback")
                    })
                    .collect::<Vec<_>>();
                assert_eq!(guidance.len(), 1, "fallback guidance is released exactly once");
                assert!(guidance[0].content.contains("one simple discovery command"));
                assert!(guidance[0].content.contains("ordinary read tool"));
                for private in provider_values("qualified_name") {
                    assert!(!guidance[0].content.contains(&private));
                }
                record(DecisionStep::GraphRetry);
                tool_reply(
                    "closed-graph-retry",
                    "codebase_memory_search_graph",
                    serde_json::json!({"query": "must-remain-closed"}),
                )
            }
            (DecisionCase::AllRootsNonViableFallback, 12) => {
                assert_eq!(
                    result_count(),
                    9,
                    "the denied graph retry never reached the provider"
                );
                assert!(view.messages.iter().any(|message| {
                    message.role == "tool"
                        && message
                            .content
                            .contains("no compatible provider-derived recovery action remains")
                }));
                record(DecisionStep::ConventionalDiscovery);
                tool_reply(
                    "bounded-conventional-discovery",
                    "bash",
                    serde_json::json!({"command": "rg pending demo/EVIDENCE.md"}),
                )
            }
            (DecisionCase::AllRootsNonViableFallback, 13) => {
                record(DecisionStep::SourceRead);
                tool_reply(
                    "ordinary-fallback-source-read",
                    "read",
                    serde_json::json!({"path": "demo/EVIDENCE.md"}),
                )
            }
            (DecisionCase::AllRootsNonViableFallback, 14) => {
                record(DecisionStep::Mutation);
                tool_reply(
                    "bounded-fallback-mutation",
                    "write",
                    serde_json::json!({
                        "path": "demo/EVIDENCE.md",
                        "content": "bounded fallback completed\n",
                    }),
                )
            }
            (DecisionCase::AllRootsNonViableFallback, 15) => {
                record(DecisionStep::Validation);
                tool_reply(
                    "validate-bounded-fallback",
                    "bash",
                    serde_json::json!({
                        "command": "test \"$(cat demo/EVIDENCE.md)\" = 'bounded fallback completed'",
                    }),
                )
            }
            (DecisionCase::AllRootsNonViableFallback, 16) => {
                record(DecisionStep::Submission);
                tool_reply(
                    "submit-bounded-fallback",
                    "submit_for_pr",
                    serde_json::json!({"summary": "bounded fallback validated"}),
                )
            }
            (DecisionCase::AllRootsNonViableFallback, 17) => {
                assert!(view.messages.iter().any(|message| {
                    message.role == "tool"
                        && message.content.contains("submit_for_pr accepted by host")
                }));
                record(DecisionStep::Complete);
                Reply::text(
                    r#"{"title":"Complete bounded fallback","body":"Validated and submitted.","summary":"Bounded fallback completed."}"#,
                )
            }
            (DecisionCase::SelectorlessViableRootFallback, 0) => {
                record(DecisionStep::Discovery);
                tool_reply(
                    "selectorless-root",
                    "codebase_memory_search_code",
                    serde_json::json!({"pattern": "selectorless-viable-root"}),
                )
            }
            (DecisionCase::SelectorlessViableRootFallback, 1) => {
                record(DecisionStep::ImplementationSource);
                tool_reply(
                    "selectorless-implementation",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({
                        "qualified_name": next_target(),
                        "decision_evidence_kind": "implementation",
                    }),
                )
            }
            (DecisionCase::SelectorlessViableRootFallback, 2) => {
                record(DecisionStep::Trace);
                tool_reply(
                    "selectorless-trace",
                    "codebase_memory_trace_path",
                    serde_json::json!({
                        "function_name": recovery_selector("implementation_evidence_result"),
                        "mode": "calls",
                        "direction": "inbound",
                    }),
                )
            }
            (DecisionCase::SelectorlessViableRootFallback, 3) => {
                assert!(view.messages.iter().any(|message| {
                    message.role == "user"
                        && message.content.contains("minimal conventional fallback")
                }));
                assert!(!view.messages.iter().any(|message| {
                    message.role == "user" && message.content.contains("caller_traversal_result=")
                }));
                record(DecisionStep::ConventionalDiscovery);
                tool_reply(
                    "selectorless-conventional-discovery",
                    "bash",
                    serde_json::json!({"command": "rg pending demo/EVIDENCE.md"}),
                )
            }
            (DecisionCase::SelectorlessViableRootFallback, 4) => {
                record(DecisionStep::SourceRead);
                tool_reply(
                    "selectorless-source-read",
                    "read",
                    serde_json::json!({"path": "demo/EVIDENCE.md"}),
                )
            }
            (DecisionCase::SelectorlessViableRootFallback, 5) => {
                record(DecisionStep::Mutation);
                tool_reply(
                    "selectorless-mutation",
                    "write",
                    serde_json::json!({
                        "path": "demo/EVIDENCE.md",
                        "content": "selectorless fallback completed\n",
                    }),
                )
            }
            (DecisionCase::SelectorlessViableRootFallback, 6) => {
                record(DecisionStep::Validation);
                tool_reply(
                    "selectorless-validation",
                    "bash",
                    serde_json::json!({
                        "command": "test \"$(cat demo/EVIDENCE.md)\" = 'selectorless fallback completed'",
                    }),
                )
            }
            (DecisionCase::SelectorlessViableRootFallback, 7) => {
                record(DecisionStep::Submission);
                tool_reply(
                    "selectorless-submission",
                    "submit_for_pr",
                    serde_json::json!({"summary": "selectorless fallback validated"}),
                )
            }
            (DecisionCase::SelectorlessViableRootFallback, 8) => {
                assert!(view.messages.iter().any(|message| {
                    message.role == "tool"
                        && message.content.contains("submit_for_pr accepted by host")
                }));
                record(DecisionStep::Complete);
                Reply::text(
                    r#"{"title":"Complete selectorless fallback","body":"Validated and submitted.","summary":"Selectorless fallback completed."}"#,
                )
            }
            (_, turn) => panic!("unexpected model turn {turn} for {case:?}"),
        }
    }))
    .expect("start opaque decision-chain fake LLM")
}

fn tool_reply(id: &str, name: &str, args: serde_json::Value) -> Reply {
    tool_replies(&[(id, name, args)])
}

fn tool_replies(calls: &[(&str, &str, serde_json::Value)]) -> Reply {
    Reply {
        turns: calls
            .iter()
            .map(|(id, name, args)| Turn::ToolCall {
                id: (*id).to_string(),
                name: (*name).to_string(),
                args: args.clone(),
            })
            .collect(),
        usage: Default::default(),
        stop: StopReason::ToolCalls,
    }
}
