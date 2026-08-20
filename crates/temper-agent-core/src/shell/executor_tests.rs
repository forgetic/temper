//! Tests for `shell::executor`.

use super::*;
use crate::machine::{
    CodebaseMemoryTiming, DecisionAnchorLineageStageV1, DecisionAnchorLineageV1,
    DecisionAnchorTargetKindV1, SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY,
    SAFE_GRAPH_CORRELATION_DETAIL_KEY, SAFE_TOOL_FAILURE_DETAIL_KEY, ToolResultMetadata,
};
use crate::shell::tool_result::TOOL_RESULT_PREVIEW_BYTES;
use async_trait::async_trait;
use skein::lab::{LabConfig, LabRuntime};
use skein::types::Budget;
use std::collections::VecDeque;
use std::future::Future;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use temper_protocol_activity::{
    GraphCorrelationTargetKindV1, GraphCorrelationToolV1, GraphCorrelationV1,
};
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
        Ok(ToolOutput {
            content: vec![tongs::model::ContentBlock::Text(
                tongs::model::TextContent {
                    text: "bounded result".to_string(),
                    text_signature: None,
                },
            )],
            details: None,
            is_error: false,
        })
    }
}

struct InvocationFlagTool {
    invoked: Arc<AtomicBool>,
}

#[async_trait]
impl Tool for InvocationFlagTool {
    fn name(&self) -> &str {
        "codebase_memory_search_graph"
    }

    fn label(&self) -> &str {
        "graph"
    }

    fn description(&self) -> &str {
        "must not run after local denial"
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
        self.invoked.store(true, Ordering::SeqCst);
        Ok(ToolOutput {
            content: Vec::new(),
            details: None,
            is_error: false,
        })
    }
}

#[test]
fn exploration_denial_never_invokes_provider_and_emits_only_the_generic_reason() {
    const SECRET: &str = "Authorization: Bearer LOCAL-DENIAL-SECRET/src/private.rs";
    let invoked = Arc::new(AtomicBool::new(false));
    let tools = ToolRegistry::from_tools(vec![Box::new(InvocationFlagTool {
        invoked: Arc::clone(&invoked),
    })]);
    let recorder = Arc::new(Recorder::default());
    let observed = Arc::clone(&recorder);
    let call = ToolCall {
        id: "denied-graph".to_string(),
        name: "codebase_memory_search_graph".to_string(),
        arguments: serde_json::json!({"query": SECRET}),
    };
    let clock = FakeClock(Mutex::new(VecDeque::from([10, 10])));

    let output = temper_agent_io::block_on(async move {
        execute_tool(
            &tools,
            &call,
            Duration::from_secs(1),
            &CancellationToken::default(),
            Some(ToolCallDenial::GraphExplorationClosed),
            &clock,
            observed.as_ref(),
            None,
            None,
        )
        .await
        .expect("local denial returns a model-visible result")
    });

    assert!(!invoked.load(Ordering::SeqCst));
    assert!(output.output.is_error);
    assert_eq!(
        bounded_result_text(&output.output, 1024).map(|(text, _)| text),
        Some(crate::CODEBASE_MEMORY_EXPLORATION_CLOSED_MESSAGE.to_string())
    );
    let events = recorder.0.lock().expect("events");
    assert!(matches!(
        &events[0],
        AgentEvent::ToolEnd {
            status: ToolCallStatus::Failed,
            result: ToolResultMetadata {
                preview: None,
                failure: Some(failure),
                graph_correlation: None,
                decision_anchor_lineage: None,
                ..
            },
            ..
        } if failure.category == ToolFailureCategory::GraphLifecycleDenial
            && failure.reason == ToolFailureReason::ExplorationClosed
    ));
    let debug = format!("{events:?}");
    assert!(!debug.contains(SECRET));
}

#[test]
fn tool_duration_uses_the_injected_monotonic_clock() {
    let tools = ToolRegistry::from_tools(vec![Box::new(FakeTool)]);
    let clock = FakeClock(Mutex::new(VecDeque::from([100, 137])));
    let recorder = Arc::new(Recorder::default());
    let observed = Arc::clone(&recorder);
    let call = ToolCall {
        id: "call-1".to_string(),
        name: "fake".to_string(),
        arguments: serde_json::json!({}),
    };

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
            None,
        )
        .await
        .expect("tool was not cancelled")
    });
    assert!(!output.output.is_error);
    assert_eq!(output.failure, None);
    let events = recorder.0.lock().expect("events");
    assert!(matches!(
        &events[0],
        AgentEvent::ToolEnd {
            id,
            name,
            status: ToolCallStatus::Succeeded,
            duration_ms: 37,
            result: ToolResultMetadata {
                preview: Some(preview),
                bytes: 14,
                truncated: false,
                failure: None,
                codebase_memory_timing: None,
                graph_correlation: None,
                decision_anchor_lineage: None,
            },
        } if id == "call-1" && name == "fake" && preview == "bounded result"
    ));
}

#[test]
fn bounded_tool_metadata_is_utf8_safe_and_omits_structured_details() {
    let output = tongs::tools::ToolOutput {
        content: vec![tongs::model::ContentBlock::Text(
            tongs::model::TextContent {
                text: "🙂".repeat(2_000),
                text_signature: None,
            },
        )],
        details: Some(serde_json::json!({"secret": "must-not-enter-preview"})),
        is_error: false,
    };
    let metadata = bounded_tool_result("fake", &output);
    assert!(metadata.truncated);
    assert!(
        metadata
            .preview
            .as_ref()
            .is_some_and(|value| value.len() <= TOOL_RESULT_PREVIEW_BYTES)
    );
    assert!(
        !metadata
            .preview
            .as_deref()
            .unwrap_or_default()
            .contains("must-not-enter-preview")
    );
}

#[test]
fn only_codebase_memory_tools_retain_numeric_graph_timings() {
    let output = ToolOutput {
        content: Vec::new(),
        details: Some(serde_json::json!({
            "timing": {
                "readiness_wait_ms": 12,
                "graph_execution_ms": 34,
                "secret": "must-not-enter-events"
            }
        })),
        is_error: false,
    };

    assert_eq!(
        bounded_tool_result("read", &output).codebase_memory_timing,
        None
    );
    assert_eq!(
        bounded_tool_result("codebase_memory_search_graph", &output).codebase_memory_timing,
        Some(CodebaseMemoryTiming {
            readiness_wait_ms: 12,
            graph_execution_ms: 34,
        })
    );
    let malformed = ToolOutput {
        details: Some(serde_json::json!({"timing": {"readiness_wait_ms": "12"}})),
        ..output
    };
    assert_eq!(
        bounded_tool_result("codebase_memory_search_graph", &malformed).codebase_memory_timing,
        None
    );
}

#[test]
fn graph_result_metadata_and_debug_remain_content_free() {
    const SECRET: &str = "Authorization: Bearer GRAPH-RESULT-SECRET";
    let text = format!("safe readiness {SECRET}");
    let output = ToolOutput {
        content: vec![tongs::model::ContentBlock::Text(
            tongs::model::TextContent {
                text: text.clone(),
                text_signature: None,
            },
        )],
        details: None,
        is_error: false,
    };
    let metadata = bounded_tool_result("codebase_memory_search_graph", &output);
    assert_eq!(metadata.preview, None);
    assert_eq!(metadata.bytes, text.len() as u64);
    assert!(!format!("{metadata:?}").contains(SECRET));
}

#[test]
fn only_closed_graph_correlation_details_enter_trusted_metadata() {
    const SECRET: &str = "Authorization: Bearer CORE-GRAPH-CORRELATION-SECRET";
    let correlation = GraphCorrelationV1::new(
        GraphCorrelationToolV1::SearchGraph,
        GraphCorrelationTargetKindV1::GraphQuery,
        SECRET,
    )
    .expect("complete target");
    let lineage = DecisionAnchorLineageV1::new(
        "00000000-0000-4000-8000-000000000001".to_string(),
        DecisionAnchorLineageStageV1::Root,
        DecisionAnchorTargetKindV1::GraphQuery,
        [DecisionAnchorTargetKindV1::QualifiedName],
    )
    .expect("canonical lineage");
    let output = ToolOutput {
        content: Vec::new(),
        details: Some(serde_json::json!({
            SAFE_GRAPH_CORRELATION_DETAIL_KEY: correlation,
            SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY: lineage,
            "raw_argument": SECRET,
        })),
        is_error: false,
    };
    let metadata = bounded_tool_result("codebase_memory_search_graph", &output);
    assert_eq!(metadata.graph_correlation, Some(correlation.clone()));
    assert_eq!(metadata.decision_anchor_lineage, Some(lineage.clone()));
    assert!(
        !serde_json::to_string(&metadata.decision_anchor_lineage)
            .expect("lineage serializes")
            .contains(SECRET),
        "lineage retains no provider value"
    );
    assert!(
        !serde_json::to_string(&metadata.graph_correlation)
            .expect("fingerprint serializes")
            .contains(SECRET)
    );
    assert_eq!(bounded_tool_result("read", &output).graph_correlation, None);

    let mut malformed = correlation;
    malformed.target_digest = SECRET.to_string();
    let malformed_output = ToolOutput {
        details: Some(serde_json::json!({
            SAFE_GRAPH_CORRELATION_DETAIL_KEY: malformed,
        })),
        ..output
    };
    assert_eq!(
        bounded_tool_result("codebase_memory_search_graph", &malformed_output).graph_correlation,
        None
    );
    let failed_output = ToolOutput {
        is_error: true,
        ..malformed_output
    };
    assert_eq!(
        bounded_tool_result("codebase_memory_search_graph", &failed_output).graph_correlation,
        None
    );
}

#[test]
fn only_codebase_memory_category_markers_create_trusted_diagnostics() {
    let details = serde_json::json!({
        SAFE_TOOL_FAILURE_DETAIL_KEY: {
            "source": "codebase_memory",
            "category": "timeout",
            "message": "Authorization: Bearer must-not-be-trusted",
            "retryable": false
        }
    });
    let output = ToolOutput {
        content: Vec::new(),
        details: Some(details),
        is_error: true,
    };

    assert_eq!(bounded_tool_result("bash", &output).failure, None);
    let diagnostic = bounded_tool_result("codebase_memory_search_graph", &output)
        .failure
        .expect("trusted wrapper category");
    assert_eq!(diagnostic.category, ToolFailureCategory::Timeout);
    assert!(diagnostic.retryable);
    assert!(diagnostic.fallback_to_conventional_discovery);
    assert_eq!(diagnostic.message, "codebase-memory request timed out");

    let forged = ToolOutput {
        content: Vec::new(),
        details: Some(serde_json::json!({
            SAFE_TOOL_FAILURE_DETAIL_KEY: {
                "source": "other_tool",
                "category": "timeout"
            }
        })),
        is_error: true,
    };
    assert_eq!(
        bounded_tool_result("codebase_memory_search_graph", &forged).failure,
        None
    );
}

fn run_in_lab<T, F>(future: F) -> (T, u64)
where
    T: Send + 'static,
    F: Future<Output = T> + Send + 'static,
{
    let mut runtime = LabRuntime::new(LabConfig::new(7).with_auto_advance().max_steps(100_000));
    let region = runtime.state.create_root_region(Budget::INFINITE);
    let result = Arc::new(Mutex::new(None));
    let task_result = Arc::clone(&result);
    let (task_id, _handle) = runtime
        .state
        .create_task(region, Budget::INFINITE, async move {
            *task_result.lock().expect("lab result") = Some(future.await);
        })
        .expect("create lab task");
    runtime.scheduler.lock().schedule(task_id, 0);
    let report = runtime.run_with_auto_advance();
    let value = result
        .lock()
        .expect("lab result")
        .take()
        .expect("lab task completed");
    (value, report.virtual_elapsed_nanos)
}

struct DropFlag(Arc<AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

struct HungTool {
    name: &'static str,
    started: Arc<AtomicBool>,
    dropped: Arc<AtomicBool>,
    cancel_on_start: Option<CancellationToken>,
}

#[async_trait]
impl Tool for HungTool {
    fn name(&self) -> &str {
        self.name
    }

    fn label(&self) -> &str {
        self.name
    }

    fn description(&self) -> &str {
        "never resolves"
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
        let _drop_flag = DropFlag(Arc::clone(&self.dropped));
        self.started.store(true, Ordering::SeqCst);
        if let Some(cancellation) = &self.cancel_on_start {
            cancellation.cancel();
        }
        futures::future::pending::<tongs::Result<ToolOutput>>().await
    }
}

#[test]
fn cancellation_reports_quiescence_only_after_every_registered_task_settles() {
    let (cq, mut completions) = temper_agent_io::channel();
    let group = RunTaskGroup::new(cq);
    let (_first_token, first_guard) = group.register();
    let (_second_token, second_guard) = group.register();

    group.cancel_all(41, 17);
    assert!(completions.try_recv().is_none());
    drop(first_guard);
    assert!(completions.try_recv().is_none());
    drop(second_guard);
    assert!(matches!(
        completions.try_recv(),
        Some(AgentCompletion::TasksQuiesced {
            operation_generation: 41,
            batch_generation: 17,
        })
    ));
    assert!(completions.try_recv().is_none());
}

#[test]
fn codebase_memory_tool_timeout_uses_virtual_time_and_emits_safe_cancelled_boundary() {
    let started = Arc::new(AtomicBool::new(false));
    let dropped = Arc::new(AtomicBool::new(false));
    let tools = ToolRegistry::from_tools(vec![Box::new(HungTool {
        name: "codebase_memory_search_graph",
        started: Arc::clone(&started),
        dropped: Arc::clone(&dropped),
        cancel_on_start: None,
    })]);
    let events = Arc::new(Recorder::default());
    let observed = Arc::clone(&events);
    let call = ToolCall {
        id: "call-timeout".to_string(),
        name: "codebase_memory_search_graph".to_string(),
        arguments: serde_json::json!({}),
    };

    let (output, virtual_elapsed) = run_in_lab(async move {
        execute_tool(
            &tools,
            &call,
            Duration::from_secs(7),
            &CancellationToken::default(),
            None,
            &SystemEventClock,
            observed.as_ref(),
            None,
            None,
        )
        .await
        .expect("a timeout is returned to the model")
    });

    assert_eq!(virtual_elapsed, Duration::from_secs(7).as_nanos() as u64);
    assert!(started.load(Ordering::SeqCst));
    assert!(dropped.load(Ordering::SeqCst));
    assert!(output.output.is_error);
    assert_eq!(
        output.output.content.iter().find_map(|block| match block {
            tongs::model::ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        }),
        Some(
            "codebase-memory request timed out; do not retry codebase-memory immediately; continue with read, grep, find, shell, or other conventional discovery instead"
        )
    );
    let events = events.0.lock().expect("events");
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0],
        AgentEvent::ToolEnd {
            id,
            name,
            status: ToolCallStatus::Cancelled,
            duration_ms: 7_000,
            result: ToolResultMetadata {
                failure: Some(failure),
                ..
            },
        } if id == "call-timeout"
            && name == "codebase_memory_search_graph"
            && failure.category == ToolFailureCategory::Timeout
            && failure.retryable
            && failure.fallback_to_conventional_discovery
            && failure.message == "codebase-memory request timed out"
    ));
}

#[test]
fn ordinary_tool_timeout_is_typed_and_model_visible_without_runtime_values() {
    let started = Arc::new(AtomicBool::new(false));
    let dropped = Arc::new(AtomicBool::new(false));
    let tools = ToolRegistry::from_tools(vec![Box::new(HungTool {
        name: "ordinary_hung",
        started: Arc::clone(&started),
        dropped: Arc::clone(&dropped),
        cancel_on_start: None,
    })]);
    let events = Arc::new(Recorder::default());
    let observed = Arc::clone(&events);
    let call = ToolCall {
        id: "ordinary-timeout".to_string(),
        name: "ordinary_hung".to_string(),
        arguments: serde_json::json!({"secret": "TIMEOUT-SECRET"}),
    };

    let (output, virtual_elapsed) = run_in_lab(async move {
        execute_tool(
            &tools,
            &call,
            Duration::from_secs(3),
            &CancellationToken::default(),
            None,
            &SystemEventClock,
            observed.as_ref(),
            None,
            None,
        )
        .await
        .expect("timeout returns a model result")
    });

    assert_eq!(virtual_elapsed, Duration::from_secs(3).as_nanos() as u64);
    let failure = output.failure.expect("typed timeout");
    assert_eq!(failure.category, ToolFailureCategory::Timeout);
    assert_eq!(failure.reason, ToolFailureReason::DeadlineExceeded);
    assert_eq!(
        failure.retry_disposition,
        crate::ToolRetryDisposition::Retryable
    );
    let rendered = format!("{failure:?} {:?}", events.0.lock().unwrap());
    assert!(!rendered.contains("TIMEOUT-SECRET"));
    assert!(started.load(Ordering::SeqCst));
    assert!(dropped.load(Ordering::SeqCst));
}

#[test]
fn external_cancellation_drops_a_hung_tool_without_advancing_time() {
    let cancellation = CancellationToken::default();
    let started = Arc::new(AtomicBool::new(false));
    let dropped = Arc::new(AtomicBool::new(false));
    let tools = ToolRegistry::from_tools(vec![Box::new(HungTool {
        name: "submit_for_pr",
        started: Arc::clone(&started),
        dropped: Arc::clone(&dropped),
        cancel_on_start: Some(cancellation.clone()),
    })]);
    let events = Arc::new(Recorder::default());
    let observed = Arc::clone(&events);
    let call = ToolCall {
        id: "call-cancel".to_string(),
        name: "submit_for_pr".to_string(),
        arguments: serde_json::json!({}),
    };

    let (output, virtual_elapsed) = run_in_lab(async move {
        execute_tool(
            &tools,
            &call,
            Duration::from_secs(600),
            &cancellation,
            None,
            &SystemEventClock,
            observed.as_ref(),
            None,
            None,
        )
        .await
    });

    assert_eq!(virtual_elapsed, 0);
    assert!(output.is_none());
    assert!(started.load(Ordering::SeqCst));
    assert!(dropped.load(Ordering::SeqCst));
    let events = events.0.lock().expect("events");
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0],
        AgentEvent::ToolEnd {
            status: ToolCallStatus::Cancelled,
            duration_ms: 0,
            result: ToolResultMetadata {
                failure: Some(failure),
                ..
            },
            ..
        } if failure.category == ToolFailureCategory::Cancellation
            && failure.reason == ToolFailureReason::RunCancelled
            && failure.retry_disposition == crate::ToolRetryDisposition::DoNotRetry
    ));
}
