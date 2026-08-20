//! Provider-decoded invocation canonicalization and pre-dispatch validation.
//!
//! The fixtures intentionally mirror the exact unified forms emitted by the
//! pinned tongs adapters: OpenAI-compatible `tool_calls[].function`, Codex
//! Responses `function_call`, and Anthropic `tool_use` all arrive as a
//! `ToolCall`; Anthropic OAuth may additionally use Claude Code names and
//! native filesystem keys.

use std::sync::Arc;

use async_trait::async_trait;
use temper_agent_io::{EngineTime, Machine};
use tongs::model::{AssistantMessage, ContentBlock, Message, StopReason, ToolCall, Usage};
use tongs::tools::{Tool, ToolEffects, ToolOutput, ToolRegistry, ToolUpdate};

use super::common::{complete, llm_responded, run_tools, tool_finished, tool_output, user};
use crate::machine::{AgentEvent, AgentMachine, AgentRequest, ToolFailureReason};
use crate::{REJECTED_TOOL_NAME, ToolInvocationCatalog};

struct ContractTool {
    name: &'static str,
    schema: serde_json::Value,
    effects: ToolEffects,
}

#[async_trait]
impl Tool for ContractTool {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        "invocation contract fixture"
    }
    fn parameters(&self) -> serde_json::Value {
        self.schema.clone()
    }
    fn effects(&self) -> ToolEffects {
        self.effects
    }
    async fn execute(
        &self,
        _: &str,
        _: serde_json::Value,
        _: Option<Box<dyn Fn(ToolUpdate) + Send + Sync>>,
    ) -> tongs::Result<ToolOutput> {
        unreachable!("pure machine tests do not execute tools")
    }
}

fn catalog(names: &[&'static str]) -> Arc<ToolInvocationCatalog> {
    let tools = names
        .iter()
        .map(|name| {
            let (schema, effects) = match *name {
                "read" => (
                    serde_json::json!({
                        "type":"object",
                        "properties":{"path":{"type":"string"},"offset":{"type":"number"}},
                        "required":["path"]
                    }),
                    ToolEffects::read(),
                ),
                "write" => (
                    serde_json::json!({
                        "type":"object",
                        "properties":{"path":{"type":"string"},"content":{"type":"string"}},
                        "required":["path","content"]
                    }),
                    ToolEffects::write(),
                ),
                "edit" => (
                    serde_json::json!({
                        "type":"object",
                        "properties":{"path":{"type":"string"},"edits":{"type":"array","items":{
                            "type":"object","properties":{"oldText":{"type":"string"},"newText":{"type":"string"}},
                            "required":["oldText","newText"]}}},
                        "required":["path","edits"]
                    }),
                    ToolEffects::write(),
                ),
                "codebase_memory_search_graph" => (
                    serde_json::json!({
                        "type":"object",
                        "properties":{"query":{"type":"string"}},
                        "required":["query"]
                    }),
                    ToolEffects::read(),
                ),
                other => panic!("unsupported fixture tool {other}"),
            };
            Box::new(ContractTool {
                name,
                schema,
                effects,
            }) as Box<dyn Tool>
        })
        .collect();
    Arc::new(
        ToolInvocationCatalog::from_registry(&ToolRegistry::from_tools(tools))
            .expect("fixture catalog"),
    )
}

fn assistant(api: &str, calls: Vec<(&str, &str, serde_json::Value)>) -> AssistantMessage {
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
        api: api.to_string(),
        provider: "provider-fixture".to_string(),
        model: "model-fixture".to_string(),
        usage: Usage::default(),
        stop_reason: StopReason::ToolUse,
        error_message: None,
        timestamp: 0,
    }
}

fn machine(catalog: Arc<ToolInvocationCatalog>) -> AgentMachine {
    AgentMachine::with_invocation_catalog(vec![user("invoke")], 10, catalog)
}

fn dispatched(requests: &[AgentRequest]) -> (&ToolCall, Option<&crate::ToolFailureDiagnostic>) {
    requests
        .iter()
        .find_map(|request| match request {
            AgentRequest::RunTool {
                call, rejection, ..
            } => Some((call, rejection.as_ref())),
            _ => None,
        })
        .expect("one dispatched call")
}

#[test]
fn accepts_exact_pinned_openai_codex_and_anthropic_canonical_forms() {
    for api in [
        "openai-completions", // tool_calls[].function.{name,arguments}
        "openai-responses",   // function_call.{name,arguments}
        "anthropic-messages", // tool_use.{name,input}
    ] {
        let mut machine = machine(catalog(&["read"]));
        let _ = machine.on_start(EngineTime::ZERO);
        let requests = complete(
            &mut machine,
            llm_responded(assistant(
                api,
                vec![("call", "read", serde_json::json!({"path":"src/lib.rs"}))],
            )),
        );
        let (call, rejection) = dispatched(&requests);
        assert_eq!(call.name, "read", "api={api}");
        assert_eq!(call.arguments["path"], "src/lib.rs", "api={api}");
        assert!(rejection.is_none(), "api={api}");
    }
}

#[test]
fn anthropic_oauth_names_and_reviewed_native_keys_become_canonical_everywhere() {
    let catalog = catalog(&["read"]);
    assert_eq!(catalog.telemetry_name("anthropic-messages", "Read"), "read");
    assert_eq!(
        catalog.telemetry_name("anthropic-messages", "unavailable-secret-name"),
        REJECTED_TOOL_NAME
    );
    let mut machine = machine(catalog);
    let _ = machine.on_start(EngineTime::ZERO);
    let requests = complete(
        &mut machine,
        llm_responded(assistant(
            "anthropic-messages",
            vec![(
                "call",
                "Read",
                serde_json::json!({"file_path":"src/lib.rs","offset":2}),
            )],
        )),
    );
    let (call, rejection) = dispatched(&requests);
    assert_eq!(call.name, "read");
    assert_eq!(
        call.arguments,
        serde_json::json!({"path":"src/lib.rs","offset":2})
    );
    assert!(rejection.is_none());

    let emitted_name = requests.iter().find_map(|request| match request {
        AgentRequest::Emit(AgentEvent::AssistantMessage { content }) => {
            content.iter().find_map(|block| match block {
                ContentBlock::ToolCall(call) => Some(call.name.as_str()),
                _ => None,
            })
        }
        _ => None,
    });
    assert_eq!(emitted_name, Some("read"));
    let retained_name = machine.messages().iter().find_map(|message| match message {
        Message::Assistant(message) => message.content.iter().find_map(|block| match block {
            ContentBlock::ToolCall(call) => Some(call.name.as_str()),
            _ => None,
        }),
        _ => None,
    });
    assert_eq!(retained_name, Some("read"));
}

#[test]
fn anthropic_native_edit_is_a_single_explicit_canonical_replacement() {
    let mut machine = machine(catalog(&["edit"]));
    let _ = machine.on_start(EngineTime::ZERO);
    let requests = complete(
        &mut machine,
        llm_responded(assistant(
            "anthropic-messages",
            vec![(
                "call",
                "Edit",
                serde_json::json!({
                    "file_path":"src/lib.rs",
                    "old_string":"old",
                    "new_string":"new"
                }),
            )],
        )),
    );
    let (call, rejection) = dispatched(&requests);
    assert_eq!(call.name, "edit");
    assert_eq!(
        call.arguments,
        serde_json::json!({
            "path":"src/lib.rs",
            "edits":[{"oldText":"old","newText":"new"}]
        })
    );
    assert!(rejection.is_none());
}

#[test]
fn malformed_ambiguous_and_unavailable_forms_are_scrubbed_and_typed() {
    let cases = [
        (
            "openai-completions",
            "read",
            serde_json::json!({}), // pinned decoder fallback for malformed JSON
            ToolFailureReason::InvalidArguments,
        ),
        (
            "openai-responses",
            "read",
            serde_json::json!({"path":7}),
            ToolFailureReason::InvalidArguments,
        ),
        (
            "anthropic-messages",
            "Read",
            serde_json::json!({"path":"a","file_path":"b"}),
            ToolFailureReason::InvalidArguments,
        ),
        (
            "anthropic-messages",
            "READ",
            serde_json::json!({"path":"a"}),
            ToolFailureReason::UnknownTool,
        ),
        (
            "anthropic-messages",
            "Write", // optional mutation tool is absent from this catalog
            serde_json::json!({"file_path":"a","content":"secret"}),
            ToolFailureReason::UnknownTool,
        ),
        (
            "openai-completions",
            "read",
            serde_json::json!({"path":"a","unknown":"secret"}),
            ToolFailureReason::InvalidArguments,
        ),
    ];
    for (api, name, arguments, expected) in cases {
        let mut machine = machine(catalog(&["read"]));
        let _ = machine.on_start(EngineTime::ZERO);
        let requests = complete(
            &mut machine,
            llm_responded(assistant(api, vec![("call", name, arguments)])),
        );
        let (call, rejection) = dispatched(&requests);
        assert_eq!(call.name, REJECTED_TOOL_NAME);
        assert_eq!(call.arguments, serde_json::json!({}));
        assert_eq!(rejection.expect("typed rejection").reason, expected);
    }
}

#[test]
fn legacy_edit_parser_compatibility_is_not_part_of_the_canonical_contract() {
    for arguments in [
        serde_json::json!({"path":"f","oldText":"a","newText":"b"}),
        serde_json::json!({"path":"f","edits":"[{\"oldText\":\"a\",\"newText\":\"b\"}]"}),
        serde_json::json!({"path":"f","edits":[{"oldText":"a","newText":"b","extra":true}]}),
    ] {
        let mut machine = machine(catalog(&["edit"]));
        let _ = machine.on_start(EngineTime::ZERO);
        let requests = complete(
            &mut machine,
            llm_responded(assistant(
                "openai-completions",
                vec![("call", "edit", arguments)],
            )),
        );
        let (_, rejection) = dispatched(&requests);
        assert_eq!(
            rejection.expect("legacy form rejected").reason,
            ToolFailureReason::InvalidArguments
        );
    }
}

#[test]
fn canonical_effects_control_alias_batching_before_dispatch() {
    let mut machine = machine(catalog(&["read", "write"]));
    let _ = machine.on_start(EngineTime::ZERO);
    let first = complete(
        &mut machine,
        llm_responded(assistant(
            "anthropic-messages",
            vec![
                ("read-call", "Read", serde_json::json!({"file_path":"a"})),
                (
                    "write-call",
                    "Write",
                    serde_json::json!({"file_path":"b","content":"c"}),
                ),
            ],
        )),
    );
    assert_eq!(run_tools(&first), ["read-call"]);
    let second = complete(
        &mut machine,
        tool_finished("read-call", tool_output("read", false)),
    );
    assert_eq!(run_tools(&second), ["write-call"]);
    let (call, rejection) = dispatched(&second);
    assert_eq!(call.name, "write");
    assert!(rejection.is_none());
}

#[test]
fn anthropic_mutation_alias_cannot_bypass_decision_anchor_policy() {
    use crate::{SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY, SAFE_GRAPH_CORRELATION_DETAIL_KEY};
    use temper_protocol_activity::{
        DecisionAnchorLineageStageV1, DecisionAnchorLineageV1, DecisionAnchorTargetKindV1,
        GraphCorrelationTargetKindV1, GraphCorrelationToolV1, GraphCorrelationV1,
    };

    let mut machine = machine(catalog(&["codebase_memory_search_graph", "write"]));
    let _ = machine.on_start(EngineTime::ZERO);
    let root = complete(
        &mut machine,
        llm_responded(assistant(
            "openai-responses",
            vec![(
                "root",
                "codebase_memory_search_graph",
                serde_json::json!({"query":"implementation"}),
            )],
        )),
    );
    assert_eq!(run_tools(&root), ["root"]);
    let correlation = GraphCorrelationV1::new(
        GraphCorrelationToolV1::SearchGraph,
        GraphCorrelationTargetKindV1::GraphQuery,
        "implementation",
    )
    .unwrap();
    let lineage = DecisionAnchorLineageV1::new(
        "00000000-0000-4000-8000-000000000001".to_string(),
        DecisionAnchorLineageStageV1::Root,
        DecisionAnchorTargetKindV1::from_graph_correlation(
            GraphCorrelationTargetKindV1::GraphQuery,
        ),
        [DecisionAnchorTargetKindV1::QualifiedName],
    )
    .unwrap();
    let _ = complete(
        &mut machine,
        tool_finished(
            "root",
            ToolOutput {
                content: Vec::new(),
                details: Some(serde_json::json!({
                    SAFE_GRAPH_CORRELATION_DETAIL_KEY: correlation,
                    SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY: lineage,
                })),
                is_error: false,
            },
        ),
    );

    let mutation = complete(
        &mut machine,
        llm_responded(assistant(
            "anthropic-messages",
            vec![(
                "mutation",
                "Write",
                serde_json::json!({"file_path":"a","content":"b"}),
            )],
        )),
    );
    assert!(mutation.iter().any(|request| matches!(
        request,
        AgentRequest::RunTool {
            call,
            mutation_blocked: true,
            rejection: None,
            ..
        } if call.name == "write"
    )));
}
