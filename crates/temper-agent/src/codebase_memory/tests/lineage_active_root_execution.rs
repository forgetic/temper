use std::sync::Arc;

use temper_agent_core::{
    AgentCompletion, AgentEvent, AgentMachine, AgentRequest, AgentStop, ToolCallDenial,
    ToolFailureCategory, ToolFailureDiagnostic, ToolInvocationCatalog,
    SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY,
};
use temper_agent_io::{EngineTime, Machine};
use temper_protocol_activity::{
    DecisionAnchorLineageV1, DecisionEvidenceKindV1, GraphRecoveryReferenceDispositionV1,
};
use temper_protocol_agent::{CodebaseMemoryIndex, CodebaseMemoryMode};
use tongs::model::{
    AssistantMessage, ContentBlock, Message, StopReason, ToolCall, Usage, UserContent, UserMessage,
};
use tongs::tools::{ToolOutput, ToolRegistry};

const REFERENCE_PREFIX: &str = "temper-recovery-selector:";

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

fn handoff_reference(handoff: &str) -> &str {
    handoff
        .split_once("\"qualified_name\":\"")
        .and_then(|(_, value)| value.split_once('"'))
        .map(|(reference, _)| reference)
        .filter(|reference| reference.starts_with(REFERENCE_PREFIX))
        .expect("opaque qualified-name reference")
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

fn local_failure(requests: &[AgentRequest], id: &str) -> ToolFailureDiagnostic {
    requests
        .iter()
        .find_map(|request| match request {
            AgentRequest::RunTool {
                call,
                denial: Some(ToolCallDenial::GraphExplorationClosed(Some(details))),
                ..
            } if call.id == id => Some(ToolFailureDiagnostic::graph_exploration(details.clone())),
            AgentRequest::RunTool {
                call,
                rejection: Some(failure),
                ..
            } if call.id == id => Some(failure.clone()),
            _ => None,
        })
        .expect("negative selector is rejected locally")
}

fn failed_output() -> ToolOutput {
    ToolOutput {
        content: Vec::new(),
        details: None,
        is_error: true,
    }
}

#[test]
fn published_parallel_active_root_handoff_executes_once_at_the_production_boundary() {
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
        let toolset = crate::codebase_memory::build_codebase_memory_toolset(
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
        let registry = ToolRegistry::from_tools(toolset.into_tools());
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
        assert!(!sibling_references.is_empty());
        assert!(complete_tool(&mut machine, "sibling-root", sibling_output, None).is_empty());

        let active_output = search
            .execute(&active_call.id, active_call.arguments, None)
            .await
            .unwrap();
        let active_root = lineage(&active_output);
        let active_references = references(&active_output);
        let selected = complete_tool(&mut machine, "active-root", active_output, None);
        assert_ne!(active_root.root_binding, sibling_root.root_binding);
        let handoff = active_handoff(&selected);
        let active_reference = handoff_reference(handoff).to_string();
        assert_eq!(active_reference, active_references[0]);
        assert!(!sibling_references.contains(&active_reference));
        for private in [
            "worker_slot",
            active_root.root_binding.as_str(),
            sibling_root.root_binding.as_str(),
        ] {
            assert!(!handoff.contains(private));
        }

        let missing_purpose = complete_llm(
            &mut machine,
            assistant(vec![(
                "missing-purpose",
                "codebase_memory_get_code_snippet",
                serde_json::json!({"qualified_name":active_reference}),
            )]),
        );
        assert!(missing_purpose.iter().any(|request| matches!(
            request,
            AgentRequest::Emit(AgentEvent::ToolStart {
                id,
                recovery_reference_disposition: Some(
                    GraphRecoveryReferenceDispositionV1::Rejected
                ),
                ..
            }) if id == "missing-purpose"
        )));
        let missing_call = missing_purpose
            .iter()
            .find_map(|request| match request {
                AgentRequest::RunTool {
                    call,
                    denial: None,
                    rejection: None,
                    ..
                } if call.id == "missing-purpose" => Some(call.clone()),
                _ => None,
            })
            .expect("a schema-valid incomplete handoff reaches wrapper validation");
        let source = registry
            .get("codebase_memory_get_code_snippet")
            .unwrap();
        let missing_output = source
            .execute(&missing_call.id, missing_call.arguments, None)
            .await
            .unwrap();
        assert!(missing_output.is_error);
        let correction = complete_tool(
            &mut machine,
            "missing-purpose",
            missing_output,
            Some(ToolFailureDiagnostic::codebase_memory(
                ToolFailureCategory::InvalidModelInput,
            )),
        );
        let correction = active_handoff(&correction);
        assert_eq!(handoff_reference(correction), active_reference);
        assert_eq!(correction.matches(REFERENCE_PREFIX).count(), 1);

        let fabricated = "temper-recovery-selector:00000000-0000-4000-8000-000000000099";
        let alternate = active_references[1].clone();
        let negatives = [
            (
                "raw",
                serde_json::json!({"qualified_name":"worker_slot","decision_evidence_kind":"implementation"}),
            ),
            (
                "alternate",
                serde_json::json!({"qualified_name":alternate,"decision_evidence_kind":"implementation"}),
            ),
            (
                "fabricated",
                serde_json::json!({"qualified_name":fabricated,"decision_evidence_kind":"implementation"}),
            ),
            (
                "sibling",
                serde_json::json!({"qualified_name":sibling_references[0],"decision_evidence_kind":"implementation"}),
            ),
            (
                "wrong-field",
                serde_json::json!({"function_name":active_reference,"decision_evidence_kind":"implementation"}),
            ),
        ];
        for (id, arguments) in negatives {
            let rejected = complete_llm(
                &mut machine,
                assistant(vec![(id, "codebase_memory_get_code_snippet", arguments)]),
            );
            let failure = local_failure(&rejected, id);
            let correction = complete_tool(&mut machine, id, failed_output(), Some(failure));
            let correction = active_handoff(&correction);
            assert_eq!(handoff_reference(correction), active_reference);
            assert_eq!(correction.matches(REFERENCE_PREFIX).count(), 1);
            assert!(!correction.contains(fabricated));
            assert!(!correction.contains(&sibling_references[0]));
            assert!(!correction.contains("worker_slot"));
            assert!(correction.contains("codebase_memory_get_code_snippet"));
            assert!(correction.contains("selector field=qualified_name"));
        }

        assert!(
            crate::codebase_memory::tests::test_support::calls_named(
                &log_path,
                "get_code_snippet"
            )
            .is_empty()
        );

        let exact = complete_llm(
            &mut machine,
            assistant(vec![(
                "exact-active",
                "codebase_memory_get_code_snippet",
                serde_json::json!({
                    "qualified_name": active_reference,
                    "decision_evidence_kind": "implementation"
                }),
            )]),
        );
        assert!(exact.iter().any(|request| matches!(
            request,
            AgentRequest::Emit(AgentEvent::ToolStart {
                id,
                recovery_reference_disposition: Some(
                    GraphRecoveryReferenceDispositionV1::Recognized
                ),
                ..
            }) if id == "exact-active"
        )));
        let exact_call = exact
            .iter()
            .find_map(|request| match request {
                AgentRequest::RunTool {
                    call,
                    denial: None,
                    rejection: None,
                    ..
                } if call.id == "exact-active" => Some(call.clone()),
                _ => None,
            })
            .expect("the exact published handoff reaches the provider");
        let source = registry
            .get("codebase_memory_get_code_snippet")
            .unwrap();
        let source_output = source
            .execute(&exact_call.id, exact_call.arguments, None)
            .await
            .unwrap();
        assert!(!source_output.is_error);
        let source_lineage = lineage(&source_output);
        assert_eq!(source_lineage.root_binding, active_root.root_binding);
        assert_eq!(
            source_lineage.decision_evidence_kind,
            Some(DecisionEvidenceKindV1::Implementation)
        );
        let source_calls = crate::codebase_memory::tests::test_support::calls_named(
            &log_path,
            "get_code_snippet",
        );
        assert_eq!(source_calls.len(), 1);
        assert_eq!(
            source_calls[0]["arguments"]["qualified_name"],
            "worker_slot"
        );
        assert!(
            source_calls[0]["arguments"]
                .get("decision_evidence_kind")
                .is_none()
        );

        let advanced = complete_tool(&mut machine, "exact-active", source_output, None);
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
