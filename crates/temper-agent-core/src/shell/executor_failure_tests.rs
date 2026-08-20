//! Ordinary tool-failure classification and privacy regressions.

use super::*;
use async_trait::async_trait;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tongs::tools::{Tool, ToolEffects, ToolOutput, ToolUpdate};

struct FakeClock(Mutex<VecDeque<u64>>);

impl EventClock for FakeClock {
    fn now_millis(&self) -> u64 {
        self.0
            .lock()
            .expect("clock")
            .pop_front()
            .expect("clock value")
    }
}

#[derive(Default)]
struct Recorder(Mutex<Vec<AgentEvent>>);

impl EventSink for Recorder {
    fn emit(&self, event: AgentEvent) {
        self.0.lock().expect("events").push(event);
    }
}

struct FakeTool;

#[async_trait]
impl Tool for FakeTool {
    fn name(&self) -> &str {
        "fake"
    }
    fn label(&self) -> &str {
        "fake"
    }
    fn description(&self) -> &str {
        "deterministic fake tool"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object"})
    }
    fn effects(&self) -> ToolEffects {
        ToolEffects::read()
    }
    async fn execute(
        &self,
        _tool_call_id: &str,
        _input: serde_json::Value,
        _on_update: Option<Box<dyn Fn(ToolUpdate) + Send + Sync>>,
    ) -> tongs::Result<ToolOutput> {
        Ok(ToolOutput::text("bounded result"))
    }
}

struct DecodeFailureTool;

#[async_trait]
impl Tool for DecodeFailureTool {
    fn name(&self) -> &str {
        "decode_failure"
    }
    fn label(&self) -> &str {
        "decode_failure"
    }
    fn description(&self) -> &str {
        "returns a secret-shaped parsing error"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"]
        })
    }
    fn effects(&self) -> ToolEffects {
        ToolEffects::read()
    }
    async fn execute(
        &self,
        _tool_call_id: &str,
        _input: serde_json::Value,
        _on_update: Option<Box<dyn Fn(ToolUpdate) + Send + Sync>>,
    ) -> tongs::Result<ToolOutput> {
        Err(tongs::error::Error::Tool(
            "Authorization: Bearer SCHEMA-SECRET".to_string(),
        ))
    }
}

struct ReportedFailureTool;

#[async_trait]
impl Tool for ReportedFailureTool {
    fn name(&self) -> &str {
        "reported_failure"
    }
    fn label(&self) -> &str {
        "reported_failure"
    }
    fn description(&self) -> &str {
        "returns secret-shaped arbitrary error output"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object"})
    }
    fn effects(&self) -> ToolEffects {
        ToolEffects::read()
    }
    async fn execute(
        &self,
        _tool_call_id: &str,
        _input: serde_json::Value,
        _on_update: Option<Box<dyn Fn(ToolUpdate) + Send + Sync>>,
    ) -> tongs::Result<ToolOutput> {
        Ok(ToolOutput {
            content: vec![tongs::model::ContentBlock::Text(
                tongs::model::TextContent {
                    text: "stderr Authorization: Bearer EXECUTION-SECRET /private/path".into(),
                    text_signature: None,
                },
            )],
            details: Some(serde_json::json!({
                "credential": "EXECUTION-SECRET",
                "provider_payload": {"raw": true}
            })),
            is_error: true,
        })
    }
}

struct PolicyFailureTool;

#[async_trait]
impl Tool for PolicyFailureTool {
    fn name(&self) -> &str {
        "forge_get_item"
    }
    fn label(&self) -> &str {
        "forge_get_item"
    }
    fn description(&self) -> &str {
        "returns a closed first-party policy code"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object"})
    }
    fn effects(&self) -> ToolEffects {
        ToolEffects::read()
    }
    async fn execute(
        &self,
        _tool_call_id: &str,
        _input: serde_json::Value,
        _on_update: Option<Box<dyn Fn(ToolUpdate) + Send + Sync>>,
    ) -> tongs::Result<ToolOutput> {
        Ok(ToolOutput {
            content: vec![],
            details: Some(serde_json::json!({
                "code": "not_authorized",
                "host_payload": "POLICY-SECRET"
            })),
            is_error: true,
        })
    }
}

fn execute_immediate(
    tools: ToolRegistry,
    name: &str,
    mutation_blocked: bool,
) -> (ExecutedTool, AgentEvent) {
    let clock = FakeClock(Mutex::new(VecDeque::from([10, 10])));
    let recorder = Arc::new(Recorder::default());
    let observed = Arc::clone(&recorder);
    let call = ToolCall {
        id: format!("call-{name}"),
        name: name.to_string(),
        arguments: serde_json::json!({}),
    };
    let output = temper_agent_io::block_on(async move {
        execute_tool(
            &tools,
            &call,
            Duration::from_secs(1),
            &CancellationToken::default(),
            mutation_blocked.then_some(ToolCallDenial::DecisionAnchorMutation),
            &clock,
            observed.as_ref(),
            None,
            None,
        )
        .await
        .expect("immediate tool settles")
    });
    let event = recorder.0.lock().expect("events").remove(0);
    (output, event)
}

#[test]
fn shell_owns_schema_policy_and_execution_classification_without_raw_values() {
    let cases = [
        (
            execute_immediate(
                ToolRegistry::from_tools(vec![Box::new(FakeTool)]),
                "unregistered_secret_tool",
                false,
            ),
            ToolFailureCategory::SchemaArgumentMismatch,
            ToolFailureReason::UnknownTool,
        ),
        (
            execute_immediate(
                ToolRegistry::from_tools(vec![Box::new(DecodeFailureTool)]),
                "decode_failure",
                false,
            ),
            ToolFailureCategory::SchemaArgumentMismatch,
            ToolFailureReason::InvalidArguments,
        ),
        (
            execute_immediate(
                ToolRegistry::from_tools(vec![Box::new(FakeTool)]),
                "fake",
                true,
            ),
            ToolFailureCategory::PolicyDenial,
            ToolFailureReason::PolicyPrecondition,
        ),
        (
            execute_immediate(
                ToolRegistry::from_tools(vec![Box::new(PolicyFailureTool)]),
                "forge_get_item",
                false,
            ),
            ToolFailureCategory::PolicyDenial,
            ToolFailureReason::AccessDenied,
        ),
        (
            execute_immediate(
                ToolRegistry::from_tools(vec![Box::new(ReportedFailureTool)]),
                "reported_failure",
                false,
            ),
            ToolFailureCategory::ExecutionFailure,
            ToolFailureReason::ToolReportedFailure,
        ),
    ];

    for ((output, event), category, reason) in cases {
        let diagnostic = output.failure.expect("failed call has diagnostic");
        assert_eq!(diagnostic.category, category);
        assert_eq!(diagnostic.reason, reason);
        assert!(output.output.is_error);
        let model_text = output.output.content.iter().find_map(|block| match block {
            tongs::model::ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        });
        assert_eq!(model_text, Some(diagnostic.message.as_str()));
        let AgentEvent::ToolEnd { result, status, .. } = event else {
            panic!("expected tool end");
        };
        assert_ne!(status, ToolCallStatus::Succeeded);
        assert_eq!(result.failure.as_ref(), Some(&diagnostic));
        let rendered = format!("{result:?} {diagnostic:?}");
        for secret in [
            "SCHEMA-SECRET",
            "EXECUTION-SECRET",
            "/private/path",
            "provider_payload",
            "POLICY-SECRET",
            "host_payload",
        ] {
            assert!(!rendered.contains(secret), "diagnostic leaked {secret}");
        }
    }
}

#[test]
fn catalog_preflight_rejection_never_executes_the_registry_tool() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct NeverRunTool(Arc<AtomicUsize>);

    #[async_trait]
    impl Tool for NeverRunTool {
        fn name(&self) -> &str {
            "read"
        }
        fn description(&self) -> &str {
            "must not execute"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type":"object"})
        }
        fn effects(&self) -> ToolEffects {
            ToolEffects::read()
        }
        async fn execute(
            &self,
            _: &str,
            _: serde_json::Value,
            _: Option<Box<dyn Fn(ToolUpdate) + Send + Sync>>,
        ) -> tongs::Result<ToolOutput> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(ToolOutput::text("unexpected"))
        }
    }

    let executions = Arc::new(AtomicUsize::new(0));
    let tools = ToolRegistry::from_tools(vec![Box::new(NeverRunTool(Arc::clone(&executions)))]);
    let clock = FakeClock(Mutex::new(VecDeque::from([10, 10])));
    let recorder = Arc::new(Recorder::default());
    let observed = Arc::clone(&recorder);
    let call = ToolCall {
        id: "rejected".to_string(),
        name: crate::REJECTED_TOOL_NAME.to_string(),
        arguments: serde_json::json!({}),
    };
    let diagnostic = ToolFailureDiagnostic::schema(ToolFailureReason::InvalidArguments);
    let output = temper_agent_io::block_on(async move {
        execute_tool(
            &tools,
            &call,
            Duration::from_secs(1),
            &CancellationToken::default(),
            None,
            &clock,
            observed.as_ref(),
            None,
            Some(diagnostic),
        )
        .await
        .expect("local rejection settles")
    });
    assert_eq!(executions.load(Ordering::SeqCst), 0);
    assert_eq!(
        output.failure.expect("typed failure").reason,
        ToolFailureReason::InvalidArguments
    );
}

#[test]
fn local_circuit_redirect_emits_canonical_failure_without_registry_or_arguments() {
    const PRIVATE_ARGUMENT: &str = "Authorization: Bearer REDIRECT-ARGUMENT";
    let failure = ToolFailureDiagnostic::new(
        ToolFailureCategory::CircuitRedirect,
        ToolFailureReason::RepeatedNonRetryable,
    );
    let call = ToolCall {
        id: "redirected".to_string(),
        name: "bash".to_string(),
        arguments: serde_json::json!({"command":PRIVATE_ARGUMENT}),
    };

    let (event, completion) = local_redirect(7, 3, call, failure.clone());
    let AgentEvent::ToolEnd {
        id,
        name,
        status,
        duration_ms,
        result,
    } = event
    else {
        panic!("expected canonical tool end");
    };
    assert_eq!(id, "redirected");
    assert_eq!(name, "bash");
    assert_eq!(status, ToolCallStatus::Failed);
    assert_eq!(duration_ms, 0);
    assert_eq!(result.failure.as_ref(), Some(&failure));
    assert_eq!(result.preview, None);
    assert!(!format!("{result:?}").contains(PRIVATE_ARGUMENT));

    let AgentCompletion::ToolFinished {
        operation_generation,
        batch_generation,
        id,
        output,
        failure: completed_failure,
    } = completion
    else {
        panic!("expected local tool completion");
    };
    assert_eq!((operation_generation, batch_generation), (7, 3));
    assert_eq!(id, "redirected");
    assert_eq!(completed_failure, Some(failure.clone()));
    let text = output.content.iter().find_map(|block| match block {
        tongs::model::ContentBlock::Text(text) => Some(text.text.as_str()),
        _ => None,
    });
    assert_eq!(text, Some(failure.message.as_str()));
    assert!(!format!("{output:?}").contains(PRIVATE_ARGUMENT));
}
