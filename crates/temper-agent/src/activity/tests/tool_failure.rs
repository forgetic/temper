use std::sync::Arc;

use temper_agent_core::{
    AgentEvent, CodebaseMemoryTiming, ModelIdentity, ToolCallStatus, ToolFailureCategory,
    ToolFailureDiagnostic, ToolResultMetadata,
};
use temper_protocol_activity::{
    AgentActivityCapturePolicyV1, AgentActivityEventV1, CaptureModeV1,
    GraphCorrelationTargetKindV1, GraphCorrelationToolV1, GraphCorrelationV1,
    ToolFailureCategoryV1,
};

use super::{FakeClock, Recorder, ScopeFactory};

#[test]
fn codebase_memory_results_and_safe_failures_follow_capture_policy() {
    const SECRET: &str = "Authorization: Bearer CODEBASE-MEMORY-SECRET";
    const CORRELATION_TARGET: &str = "activity-normalizer-CORRELATION-SECRET";
    let correlation = GraphCorrelationV1::new(
        GraphCorrelationToolV1::SearchGraph,
        GraphCorrelationTargetKindV1::GraphQuery,
        CORRELATION_TARGET,
    )
    .expect("complete correlation");
    for mode in [
        CaptureModeV1::Metadata,
        CaptureModeV1::Transcript,
        CaptureModeV1::Diagnostic,
    ] {
        let recorder = Arc::new(Recorder::default());
        let factory = ScopeFactory::with_parts(
            AgentActivityCapturePolicyV1 {
                capture: mode,
                max_inline_bytes: 64,
                ..Default::default()
            },
            Arc::new(FakeClock::new(0..20)),
            vec![recorder.clone()],
        );
        let run = factory.main("main", ModelIdentity::new("p", "m"));
        let sink = run.observability.events;
        sink.emit(AgentEvent::ToolEnd {
            id: "graph-ok".to_string(),
            name: "codebase_memory_search_graph".to_string(),
            status: ToolCallStatus::Succeeded,
            duration_ms: 4,
            result: ToolResultMetadata {
                preview: Some(format!(
                    "PRIVATE-SOURCE-PATH bounded graph evidence {}",
                    "x".repeat(200)
                )),
                bytes: 223,
                truncated: true,
                failure: None,
                codebase_memory_timing: Some(CodebaseMemoryTiming {
                    readiness_wait_ms: 1,
                    graph_execution_ms: 3,
                }),
                graph_correlation: Some(correlation.clone()),
            },
        });
        let mut failure = ToolFailureDiagnostic::codebase_memory(ToolFailureCategory::ProcessExit);
        failure.message = format!("{SECRET} {}", "y".repeat(10_000));
        sink.emit(AgentEvent::ToolEnd {
            id: "graph-failed".to_string(),
            name: "codebase_memory_search_graph".to_string(),
            status: ToolCallStatus::Failed,
            duration_ms: 5,
            result: ToolResultMetadata {
                preview: Some(format!("provider output {SECRET}")),
                bytes: 10_000,
                truncated: true,
                failure: Some(failure),
                codebase_memory_timing: Some(CodebaseMemoryTiming {
                    readiness_wait_ms: 2,
                    graph_execution_ms: 3,
                }),
                graph_correlation: Some(correlation.clone()),
            },
        });

        let frames = recorder.0.lock().expect("frames");
        let finished = frames
            .iter()
            .filter_map(|frame| match &frame.event {
                AgentActivityEventV1::ToolFinished(value) => Some(value),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(finished.len(), 2);
        assert_eq!(
            finished[0].result, None,
            "durable activity must not retain provider-shaped graph output"
        );
        assert_eq!(
            finished[0]
                .codebase_memory_timing
                .unwrap()
                .graph_execution_ms,
            3
        );
        assert_eq!(
            finished[0].graph_correlation.as_ref(),
            Some(&correlation),
            "valid fingerprints remain available even in metadata capture"
        );
        assert_eq!(finished[1].graph_correlation, None);
        assert_eq!(finished[1].result, None);
        let diagnostic = finished[1].failure.as_ref().expect("typed failure");
        assert_eq!(diagnostic.category, ToolFailureCategoryV1::ProcessExit);
        assert_eq!(
            diagnostic.message,
            "codebase-memory provider process exited"
        );
        assert!(!diagnostic.retryable);
        assert!(diagnostic.fallback_to_conventional_discovery);
        let json = serde_json::to_string(&*frames).unwrap();
        assert!(!json.contains(SECRET));
        assert!(!json.contains("PRIVATE-SOURCE-PATH"));
        assert!(!json.contains(CORRELATION_TARGET));
        assert!(!json.contains(&"y".repeat(1_000)));
    }
}
