//! Scripted model for opaque decision-chain integration cases.

use super::*;

mod partial_evidence;
mod staged_detours;

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
        let focused_test_target = || {
            view.messages
                .iter()
                .filter_map(|message| provider_result(&message.content))
                .find_map(|result| {
                    result
                        .get("results")
                        .and_then(serde_json::Value::as_array)?
                        .iter()
                        .find(|result| {
                            result
                                .get("is_test")
                                .and_then(serde_json::Value::as_bool)
                                == Some(true)
                        })
                        .and_then(|result| result.get("qualified_name"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                })
                .expect("semantic discovery omitted its exact focused test")
        };
        let source_target = |marker: &str| {
            view.messages
                .iter()
                .filter_map(|message| provider_result(&message.content))
                .find_map(|result| {
                    let source = result.pointer(&format!("/results/0/{marker}"))?;
                    source.as_str()?;
                    result.pointer("/results/0/qualified_name")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                })
                .unwrap_or_else(|| panic!("missing source target for {marker}"))
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
        let recovery_selector_from = |label: &str, result_marker: &str| {
            let marker = format!("{label}=temper-recovery-selector:");
            view.messages
                .iter()
                .find(|message| message.content.contains(result_marker))
                .and_then(|message| {
                    let start = message.content.find(&marker)? + label.len() + 1;
                    let value = &message.content[start..];
                    let end = value
                        .find([',', ' ', ']', '.'])
                        .unwrap_or(value.len());
                    Some(value[..end].to_string())
                })
                .unwrap_or_else(|| panic!("missing {label} reference on {result_marker}"))
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

        if let Some(reply) = partial_evidence::reply(
            case,
            view.prior_tool_results,
            view,
            &record,
            &next_target,
            &recovery_selector,
            &mutation_was_blocked,
        ) {
            return reply;
        }
        if let Some(reply) = staged_detours::reply(
            case,
            view.prior_tool_results,
            view,
            &record,
            &next_target,
            &focused_test_target,
            &source_target,
            &recovery_selector,
        ) {
            return reply;
        }

        match (case, view.prior_tool_results) {
            (DecisionCase::RootCoherentForest, 0) => {
                record(DecisionStep::Discovery);
                tool_replies(&[
                    (
                        "discover-forest-implementation",
                        "codebase_memory_search_graph",
                        serde_json::json!({"query": "implementation"}),
                    ),
                    (
                        "discover-forest-focused-test",
                        "codebase_memory_search_graph",
                        serde_json::json!({"query": "forest-focused-test"}),
                    ),
                ])
            }
            (DecisionCase::RootCoherentForest, 2) => {
                record(DecisionStep::Refinement);
                tool_reply(
                    "refine-forest-implementation",
                    "codebase_memory_search_code",
                    serde_json::json!({"pattern": next_target()}),
                )
            }
            (DecisionCase::RootCoherentForest, 3) => {
                record(DecisionStep::ImplementationSource);
                tool_reply(
                    "read-forest-implementation",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({
                        "qualified_name": next_target(),
                        "decision_evidence_kind": "implementation",
                    }),
                )
            }
            (DecisionCase::RootCoherentForest, 4) => {
                record(DecisionStep::Trace);
                tool_reply(
                    "trace-forest-implementation",
                    "codebase_memory_trace_path",
                    serde_json::json!({
                        "function_name": recovery_selector("implementation_evidence_result"),
                        "direction": "inbound",
                    }),
                )
            }
            (DecisionCase::RootCoherentForest, 5) => {
                record(DecisionStep::CallerSource);
                tool_reply(
                    "read-forest-caller",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({
                        "qualified_name": recovery_selector("caller_traversal_result"),
                        "decision_evidence_kind": "caller",
                    }),
                )
            }
            (DecisionCase::RootCoherentForest, 6) => {
                assert_guidance(
                    view,
                    &[
                        "active-root missing evidence=[focused_test]",
                        "selector=focused_test_result",
                    ],
                );
                record(DecisionStep::BehavioralTestSource);
                tool_reply(
                    "read-forest-focused-test",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({
                        "qualified_name": recovery_selector_from(
                            "focused_test_result",
                            "PRIVATE-PROVIDER-PAYLOAD",
                        ),
                        "decision_evidence_kind": "focused_test",
                    }),
                )
            }
            (DecisionCase::RootCoherentForest, 7) => {
                assert_guidance(
                    view,
                    &[
                        "active-root missing evidence=[]",
                        "recovery=complete",
                        "exact read",
                    ],
                );
                record(DecisionStep::SourceRead);
                tool_reply(
                    "read-forest-target",
                    "read",
                    serde_json::json!({"path": "demo/EVIDENCE.md"}),
                )
            }
            (DecisionCase::RootCoherentForest, 8) => {
                record(DecisionStep::Mutation);
                tool_reply(
                    "mutate-after-forest-evidence",
                    "write",
                    serde_json::json!({
                        "path": "demo/EVIDENCE.md",
                        "content": "verified forest evidence\n",
                    }),
                )
            }
            (DecisionCase::RootCoherentForest, 9) => {
                assert!(
                    !mutation_was_blocked(),
                    "complete forest plus exact read must admit the matching mutation",
                );
                record(DecisionStep::Complete);
                Reply::text(
                    r#"{"title":"Verify coherent decision forest","body":"Complete typed evidence preceded the exact read and mutation.","summary":"Validated coherent decision evidence."}"#,
                )
            }
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
                        "required next stage=[get_code_snippet/qualified_name/focused_test/selector=focused_test_result]",
                    ],
                );
                record(DecisionStep::BehavioralTestSource);
                tool_reply(
                    "read-provider-behavioral-test",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({
                        "qualified_name": recovery_selector("focused_test_result"),
                        "decision_evidence_kind": "focused_test",
                    }),
                )
            }
            (DecisionCase::Consumed, 6) => {
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
                    "mutation requires consumed implementation/caller and focused-test evidence"
                );
                tool_reply(
                    "read-exact-target-after-evidence",
                    "read",
                    serde_json::json!({"path": "demo/EVIDENCE.md"}),
                )
            }
            (DecisionCase::Consumed, 7) => {
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
            (DecisionCase::Consumed, 8) => {
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
                record(DecisionStep::SourceRead);
                tool_reply(
                    "read-after-unrelated-result",
                    "read",
                    serde_json::json!({"path": "demo/README.md"}),
                )
            }
            (DecisionCase::UnrelatedLaterTarget, 3) => {
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
            (DecisionCase::UnrelatedLaterTarget, 4) => {
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
                    serde_json::json!({"path": "demo/EVIDENCE.md"}),
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
            (DecisionCase::AllRootsNonViableIncomplete, 0) => {
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
            (DecisionCase::AllRootsNonViableIncomplete, count @ 3..=4) => {
                record(DecisionStep::Recovery);
                tool_reply(
                    &format!("bounded-non-progress-{count}"),
                    "codebase_memory_get_architecture",
                    serde_json::json!({}),
                )
            }
            (DecisionCase::AllRootsNonViableIncomplete, count @ 5 | count @ 7 | count @ 9) => {
                let root_index = (count - 5) / 2;
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
            (DecisionCase::AllRootsNonViableIncomplete, count @ 6 | count @ 8 | count @ 10) => {
                let root_index = (count - 6) / 2;
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
            (DecisionCase::SelectorlessViableRootIncomplete, 0) => {
                record(DecisionStep::Discovery);
                tool_reply(
                    "selectorless-root",
                    "codebase_memory_search_code",
                    serde_json::json!({"pattern": "selectorless-viable-root"}),
                )
            }
            (DecisionCase::SelectorlessViableRootIncomplete, 1) => {
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
            (DecisionCase::SelectorlessViableRootIncomplete, 2) => {
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
