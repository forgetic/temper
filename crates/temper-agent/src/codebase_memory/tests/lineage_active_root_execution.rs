use async_trait::async_trait;
use std::sync::Arc;

use temper_agent_core::{
    AgentCompletion, AgentEvent, AgentMachine, AgentRequest, AgentStop, InvocationTargetAdmission,
    LineageAdmissionOutcome, TargetAdmissionOutcome, TargetAdmissionStatus, ToolCallDenial, ToolFailureCategory,
    ToolFailureDiagnostic, ToolInvocationCatalog, SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY,
};
use temper_agent_io::{EngineTime, Machine};
use temper_protocol_activity::{
    DecisionAnchorLineageV1, DecisionEvidenceKindV1, FocusedTestDiscoveryOutcomeV1,
    GraphRecoveryReferenceDispositionV1,
};
use temper_protocol_agent::{CodebaseMemoryIndex, CodebaseMemoryMode};
use tongs::model::{
    AssistantMessage, ContentBlock, Message, StopReason, ToolCall, Usage, UserContent, UserMessage,
};
use tongs::tools::{Tool, ToolEffects, ToolOutput, ToolRegistry, ToolUpdate};

struct BlockedMutationTool;

#[async_trait]
impl Tool for BlockedMutationTool {
    fn name(&self) -> &str {
        "write"
    }

    fn description(&self) -> &str {
        "test-only mutation"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {"path": {"type": "string"}, "content": {"type": "string"}},
            "required": ["path", "content"]
        })
    }

    fn effects(&self) -> ToolEffects {
        ToolEffects::write()
    }

    async fn execute(
        &self,
        _: &str,
        _: serde_json::Value,
        _: Option<Box<dyn Fn(ToolUpdate) + Send + Sync>>,
    ) -> tongs::Result<ToolOutput> {
        unreachable!("the incomplete-evidence guard rejects this mutation")
    }
}

const REFERENCE_PREFIX: &str = "temper-recovery-selector:";
const ACTIVE_PROVIDER_SELECTOR: &str = "worker_slot";
const SIBLING_PROVIDER_SELECTOR: &str = "sibling_worker_slot";

fn assistant(calls: Vec<(&str, &str, serde_json::Value)>) -> AssistantMessage {
    AssistantMessage {
        content: calls
            .into_iter()
            .map(|(id, name, arguments)| {
                ContentBlock::ToolCall(ToolCall {
                    id: id.to_string(),
                    name: name.to_string(),
                    arguments,
                })
            })
            .collect(),
        api: "openai-responses".to_string(),
        provider: "test".to_string(),
        model: "test".to_string(),
        usage: Usage::default(),
        stop_reason: StopReason::ToolUse,
        error_message: None,
        timestamp: 0,
    }
}

fn complete_llm(machine: &mut AgentMachine, message: AssistantMessage) -> Vec<AgentRequest> {
    let (operation_generation, batch_generation) = machine.active_generations().unwrap();
    machine.on_completion(
        EngineTime::ZERO,
        AgentCompletion::LlmResponded {
            operation_generation,
            batch_generation,
            message,
        },
    )
}

fn complete_tool(
    machine: &mut AgentMachine,
    id: &str,
    output: ToolOutput,
    failure: Option<ToolFailureDiagnostic>,
) -> Vec<AgentRequest> {
    let (operation_generation, batch_generation) =
        machine.active_tool_generations(id).expect("active tool");
    machine.on_completion(
        EngineTime::ZERO,
        AgentCompletion::ToolFinished {
            operation_generation,
            batch_generation,
            id: id.to_string(),
            output,
            failure,
        },
    )
}

fn dispatched_call(requests: &[AgentRequest], id: &str) -> ToolCall {
    requests
        .iter()
        .find_map(|request| match request {
            AgentRequest::RunTool { call, .. } if call.id == id => Some(call.clone()),
            _ => None,
        })
        .expect("call dispatched")
}

fn active_handoff(requests: &[AgentRequest]) -> &str {
    requests
        .iter()
        .find_map(|request| match request {
            AgentRequest::CallLlm { messages, .. } => messages.last().and_then(|message| {
                let Message::User(message) = message else {
                    return None;
                };
                let UserContent::Text(text) = &message.content else {
                    return None;
                };
                text.contains("Active-root selector handoff")
                    .then_some(text.as_str())
            }),
            _ => None,
        })
        .expect("a fresh active-root handoff is the final correction")
}

fn handoff_references(handoff: &str) -> Vec<String> {
    handoff
        .split([',', '.', ' ', ']', '['])
        .filter_map(|part| part.split_once('='))
        .filter(|(label, value)| {
            label.starts_with("candidate_") && value.starts_with(REFERENCE_PREFIX)
        })
        .map(|(_, value)| value.to_string())
        .collect()
}

fn references(output: &ToolOutput) -> Vec<String> {
    crate::codebase_memory::tests::test_support::output_text(output)
        .split([',', '.', ' ', ']'])
        .filter_map(|part| part.split_once('='))
        .filter(|(label, value)| {
            label.starts_with("implementation_candidate")
                && value.starts_with(REFERENCE_PREFIX)
        })
        .map(|(_, value)| value.to_string())
        .collect()
}

fn lineage(output: &ToolOutput) -> DecisionAnchorLineageV1 {
    serde_json::from_value(
        output
            .details
            .as_ref()
            .and_then(|details| details.get(SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY))
            .cloned()
            .expect("typed lineage"),
    )
    .unwrap()
}

fn maybe_local_failure(
    requests: &[AgentRequest],
    id: &str,
) -> Option<ToolFailureDiagnostic> {
    requests.iter().find_map(|request| match request {
        AgentRequest::RunTool {
            call,
            denial: Some(ToolCallDenial::GraphExplorationClosed(Some(details))),
            ..
        } if call.id == id => Some(ToolFailureDiagnostic::graph_exploration(details.clone())),
        AgentRequest::RunTool {
            call,
            denial: Some(ToolCallDenial::DecisionAnchorCorrectionInspection),
            ..
        } if call.id == id => Some(ToolFailureDiagnostic::correction_inspection_denial()),
        AgentRequest::RunTool {
            call,
            rejection: Some(failure),
            ..
        } if call.id == id => Some(failure.clone()),
        _ => None,
    })
}

fn local_failure(requests: &[AgentRequest], id: &str) -> ToolFailureDiagnostic {
    maybe_local_failure(requests, id)
        .unwrap_or_else(|| panic!("negative selector {id} is rejected locally"))
}

fn failed_output() -> ToolOutput {
    ToolOutput {
        content: Vec::new(),
        details: None,
        is_error: true,
    }
}

fn run_parallel_forest_candidate_menu(reverse_completion: bool) {
    let server = crate::codebase_memory::tests::test_support::fake_server_script();
    let workspace = tempfile::tempdir().unwrap();
    let log_path = workspace.path().join("mcp.log");
    let context = crate::codebase_memory::tests::test_support::workspace_context(
        workspace.path(),
        &[("acme", "demo", "demo")],
    );
    std::fs::create_dir_all(workspace.path().join("demo/src")).unwrap();
    std::fs::write(
        workspace.path().join("demo/src/route.rs"),
        "fn worker_slot() {}\n",
    )
    .unwrap();

    temper_agent_io::block_on(async move {
        let toolset = crate::codebase_memory::tests::build_codebase_memory_toolset(
            Some(&crate::codebase_memory::tests::test_support::config(
                &server,
                CodebaseMemoryMode::Required,
                CodebaseMemoryIndex::Off,
                "active-root-handoff",
                &log_path,
                serde_json::json!({}),
            )),
            "engineer",
            &context,
            workspace.path(),
        )
        .await
        .unwrap();
        let admission = toolset.lineage_admission().unwrap();
        let mut tools = toolset.into_tools();
        tools.push(Box::new(BlockedMutationTool));
        let registry = ToolRegistry::from_tools(tools);
        let catalog = Arc::new(ToolInvocationCatalog::from_registry(&registry).unwrap());
        let mut machine = AgentMachine::with_invocation_catalog(
            vec![Message::User(UserMessage {
                content: UserContent::Text("repair routing".to_string()),
                timestamp: 0,
            })],
            12,
            catalog,
        )
        .with_lineage_admission(admission);
        let _ = machine.on_start(EngineTime::ZERO);

        let roots = complete_llm(
            &mut machine,
            assistant(vec![
                (
                    "active-root",
                    "codebase_memory_search_graph",
                    serde_json::json!({"query":"active routing implementation"}),
                ),
                (
                    "sibling-root",
                    "codebase_memory_search_graph",
                    serde_json::json!({"query":"sibling routing implementation"}),
                ),
                (
                    "blocked-mutation",
                    "write",
                    serde_json::json!({"path":"demo/src/route.rs","content":"changed"}),
                ),
            ]),
        );
        let active_call = dispatched_call(&roots, "active-root");
        let sibling_call = dispatched_call(&roots, "sibling-root");
        let search = registry.get("codebase_memory_search_graph").unwrap();

        let sibling_output = search
            .execute(&sibling_call.id, sibling_call.arguments, None)
            .await
            .unwrap();
        let sibling_root = lineage(&sibling_output);
        let sibling_references = references(&sibling_output);
        assert!(
            !sibling_references.is_empty(),
            "sibling result omitted references: {}",
            crate::codebase_memory::tests::test_support::output_text(&sibling_output)
        );
        let active_output = search
            .execute(&active_call.id, active_call.arguments, None)
            .await
            .unwrap();
        let active_root = lineage(&active_output);
        let active_references = references(&active_output);
        let blocked = if reverse_completion {
            assert!(complete_tool(&mut machine, "sibling-root", sibling_output, None).is_empty());
            complete_tool(&mut machine, "active-root", active_output, None)
        } else {
            assert!(complete_tool(&mut machine, "active-root", active_output, None).is_empty());
            complete_tool(&mut machine, "sibling-root", sibling_output, None)
        };
        assert_ne!(active_root.root_binding, sibling_root.root_binding);
        assert!(blocked.iter().any(|request| matches!(
            request,
            AgentRequest::RunTool {
                call,
                denial: Some(ToolCallDenial::DecisionAnchorMutation),
                rejection: None,
                ..
            } if call.id == "blocked-mutation"
        )));
        let selected = complete_tool(
            &mut machine,
            "blocked-mutation",
            failed_output(),
            Some(ToolFailureDiagnostic::policy_denial()),
        );
        let handoff = active_handoff(&selected);
        assert_eq!(handoff_references(handoff), active_references);
        let active_reference = active_references[0].clone();
        assert!(!sibling_references.contains(&active_reference));
        for private in [
            "worker_slot",
            active_root.root_binding.as_str(),
            sibling_root.root_binding.as_str(),
        ] {
            assert!(!handoff.contains(private));
        }

        let locally_rejected = complete_llm(
            &mut machine,
            assistant(vec![(
                "raw-source",
                "codebase_memory_get_code_snippet",
                serde_json::json!({
                    "qualified_name":"worker_slot",
                    "decision_evidence_kind":"implementation",
                    "include_neighbors":"invalid"
                }),
            )]),
        );
        let raw_failure = local_failure(&locally_rejected, "raw-source");
        assert!(locally_rejected.iter().any(|request| matches!(
            request,
            AgentRequest::Emit(AgentEvent::ToolStart {
                id,
                recovery_reference_disposition: None,
                ..
            }) if id == "raw-source"
        )));
        assert!(locally_rejected.iter().any(|request| matches!(
            request,
            AgentRequest::RunTool {
                call,
                denial: None,
                rejection: Some(_),
                ..
            } if call.id == "raw-source"
        )));
        let correction = complete_tool(
            &mut machine,
            "raw-source",
            failed_output(),
            Some(raw_failure),
        );
        let correction = active_handoff(&correction);
        assert_eq!(handoff_references(correction), active_references);
        assert_eq!(correction.matches(REFERENCE_PREFIX).count(), 4);
        assert!(correction.contains("codebase_memory_get_code_snippet"));
        assert!(correction.contains("selector field=qualified_name"));
        assert!(!correction.contains("worker_slot"));
        assert!(!correction.contains(&sibling_references[0]));
        assert!(!correction.contains("include_neighbors"));
        assert!(!correction.contains("decision_anchor_recovery_exhausted"));

        let wrong_purpose = complete_llm(
            &mut machine,
            assistant(vec![(
                "wrong-purpose",
                "codebase_memory_get_code_snippet",
                serde_json::json!({
                    "qualified_name":active_reference,
                    "decision_evidence_kind":"caller"
                }),
            )]),
        );
        assert!(wrong_purpose.iter().any(|request| matches!(
            request,
            AgentRequest::Emit(AgentEvent::ToolStart {
                id,
                recovery_reference_disposition: Some(
                    GraphRecoveryReferenceDispositionV1::Rejected
                ),
                ..
            }) if id == "wrong-purpose"
        )));
        let wrong_failure = local_failure(&wrong_purpose, "wrong-purpose");
        let correction = complete_tool(
            &mut machine,
            "wrong-purpose",
            failed_output(),
            Some(wrong_failure),
        );
        let correction = active_handoff(&correction);
        assert_eq!(handoff_references(correction), active_references);
        assert_eq!(correction.matches(REFERENCE_PREFIX).count(), 4);

        let fabricated = "temper-recovery-selector:00000000-0000-4000-8000-000000000099";
        let negatives = [
            (
                "normalized-not-exact",
                "codebase_memory_get_code_snippet",
                serde_json::json!({"qualified_name":"  worker_slot  ","decision_evidence_kind":"implementation"}),
            ),
            (
                "wrong-stage",
                "codebase_memory_get_code_snippet",
                serde_json::json!({"qualified_name":ACTIVE_PROVIDER_SELECTOR,"decision_evidence_kind":"caller"}),
            ),
            (
                "unreturned",
                "codebase_memory_get_code_snippet",
                serde_json::json!({"qualified_name":"not_returned","decision_evidence_kind":"implementation"}),
            ),
            (
                "wrong-tool",
                "codebase_memory_trace_path",
                serde_json::json!({"function_name":ACTIVE_PROVIDER_SELECTOR}),
            ),
            (
                "fabricated",
                "codebase_memory_get_code_snippet",
                serde_json::json!({"qualified_name":fabricated,"decision_evidence_kind":"implementation"}),
            ),
        ];
        let rejected = complete_llm(
            &mut machine,
            assistant(
                negatives
                    .iter()
                    .map(|(id, tool, arguments)| (*id, *tool, arguments.clone()))
                    .collect(),
            ),
        );
        let failures = negatives
            .iter()
            .map(|(id, _, _)| (*id, local_failure(&rejected, id)))
            .collect::<Vec<_>>();
        let mut correction = Vec::new();
        for (id, failure) in failures {
            correction.extend(complete_tool(
                &mut machine,
                id,
                failed_output(),
                Some(failure),
            ));
        }
        let correction = active_handoff(&correction);
        assert_eq!(handoff_references(correction), active_references);
        assert_eq!(correction.matches(REFERENCE_PREFIX).count(), 4);
        for private in [
            fabricated,
            ACTIVE_PROVIDER_SELECTOR,
            SIBLING_PROVIDER_SELECTOR,
            "active_8",
        ] {
            assert!(!correction.contains(private));
        }
        assert!(!correction.contains(&sibling_references[0]));
        assert!(correction.contains("codebase_memory_get_code_snippet"));
        assert!(correction.contains("selector field=qualified_name"));

        assert!(
            crate::codebase_memory::tests::test_support::calls_named(
                &log_path,
                "get_code_snippet"
            )
            .is_empty()
        );

        let selected_source = complete_llm(
            &mut machine,
            assistant(vec![(
                "selected-active",
                "codebase_memory_get_code_snippet",
                serde_json::json!({
                    "qualified_name": active_reference,
                    "decision_evidence_kind": "implementation"
                }),
            )]),
        );
        assert!(selected_source.iter().any(|request| matches!(
            request,
            AgentRequest::Emit(AgentEvent::ToolStart {
                id,
                recovery_reference_disposition: Some(
                    GraphRecoveryReferenceDispositionV1::Recognized
                ),
                ..
            }) if id == "selected-active"
        )));
        let selected_call = selected_source
            .iter()
            .find_map(|request| match request {
                AgentRequest::RunTool {
                    call,
                    denial: None,
                    rejection: None,
                    ..
                } if call.id == "selected-active" => Some(call.clone()),
                _ => None,
            })
            .expect("the selected active-root candidate reaches the provider");
        let source = registry
            .get("codebase_memory_get_code_snippet")
            .unwrap();
        let source_output = source
            .execute(&selected_call.id, selected_call.arguments, None)
            .await
            .unwrap();
        assert!(!source_output.is_error);
        let source_lineage = lineage(&source_output);
        assert_eq!(source_lineage.root_binding, active_root.root_binding);
        assert_eq!(
            source_lineage.decision_evidence_kind,
            Some(DecisionEvidenceKindV1::Implementation)
        );
        assert!(
            !serde_json::to_string(&source_lineage)
                .unwrap()
                .contains(ACTIVE_PROVIDER_SELECTOR)
        );
        let source_calls = crate::codebase_memory::tests::test_support::calls_named(
            &log_path,
            "get_code_snippet",
        );
        assert_eq!(source_calls.len(), 1);
        assert_eq!(
            source_calls[0]["arguments"]["qualified_name"],
            ACTIVE_PROVIDER_SELECTOR
        );
        assert!(source_calls[0]["arguments"]
            .get("decision_evidence_kind")
            .is_none());

        let advanced = complete_tool(&mut machine, "selected-active", source_output, None);
        assert!(advanced.iter().any(|request| match request {
            AgentRequest::CallLlm { messages, .. } => messages.iter().any(|message| matches!(
                message,
                Message::User(message) if matches!(&message.content, UserContent::Text(text)
                    if text.contains("accepted evidence=[implementation]"))
            )),
            _ => false,
        }));
        assert!(!advanced.iter().any(|request| matches!(
            request,
            AgentRequest::Finished {
                stop: AgentStop::DecisionAnchorRecoveryExhausted,
                ..
            }
        )));

        let blocked_after_source = complete_llm(
            &mut machine,
            assistant(vec![(
                "mutation-before-forest",
                "write",
                serde_json::json!({"path":"demo/src/route.rs","content":"changed"}),
            )]),
        );
        assert!(blocked_after_source.iter().any(|request| matches!(
            request,
            AgentRequest::RunTool {
                call,
                denial: Some(ToolCallDenial::DecisionAnchorMutation),
                rejection: None,
                ..
            } if call.id == "mutation-before-forest"
        )));
        let _ = complete_tool(
            &mut machine,
            "mutation-before-forest",
            failed_output(),
            Some(ToolFailureDiagnostic::policy_denial()),
        );

        let stale = complete_llm(
            &mut machine,
            assistant(vec![(
                "stale",
                "codebase_memory_get_code_snippet",
                serde_json::json!({
                    "qualified_name": active_reference,
                    "decision_evidence_kind": "implementation"
                }),
            )]),
        );
        let stale_failure = local_failure(&stale, "stale");
        let stale_done = complete_tool(
            &mut machine,
            "stale",
            failed_output(),
            Some(stale_failure),
        );
        assert_eq!(
            crate::codebase_memory::tests::test_support::calls_named(
                &log_path,
                "get_code_snippet"
            )
            .len(),
            1
        );
        assert!(!stale_done.iter().any(|request| matches!(
            request,
            AgentRequest::Finished {
                stop: AgentStop::DecisionAnchorRecoveryExhausted,
                ..
            }
        )));
    });
}

#[test]
fn parallel_forest_candidate_menu_is_stable_in_request_order_completion() {
    run_parallel_forest_candidate_menu(false);
}

#[test]
fn parallel_forest_candidate_menu_is_stable_in_reverse_completion() {
    run_parallel_forest_candidate_menu(true);
}
