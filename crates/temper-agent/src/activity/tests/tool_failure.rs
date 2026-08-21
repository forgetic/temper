use std::sync::Arc;

use temper_agent_core::{
    AgentEvent, CodebaseMemoryTiming, ModelIdentity, ToolCallStatus, ToolFailureCategory,
    ToolFailureDiagnostic, ToolFailureReason, ToolResultMetadata,
};
use temper_protocol_activity::{
    AgentActivityCapturePolicyV1, AgentActivityEventV1, CaptureModeV1,
    DecisionAnchorLineageStageV1, DecisionAnchorLineageV1, DecisionAnchorTargetKindV1,
    DecisionEvidenceKindV1, GraphCorrelationTargetKindV1, GraphCorrelationToolV1,
    GraphCorrelationV1, ToolFailureCategoryV1, ToolFailureReasonV1, ToolRetryDispositionV1,
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
                decision_anchor_lineage: None,
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
                decision_anchor_lineage: None,
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

#[test]
fn activity_carries_only_closed_source_decision_evidence() {
    const SECRET: &str = "Authorization: Bearer ACTIVITY-EVIDENCE-SECRET";
    let correlation = GraphCorrelationV1::new(
        GraphCorrelationToolV1::GetCodeSnippet,
        GraphCorrelationTargetKindV1::QualifiedName,
        SECRET,
    )
    .unwrap();
    let lineage = DecisionAnchorLineageV1::new_with_decision_evidence_kind(
        "00000000-0000-4000-8000-000000000001".to_string(),
        DecisionAnchorLineageStageV1::CarryForward,
        DecisionAnchorTargetKindV1::QualifiedName,
        [],
        DecisionEvidenceKindV1::Implementation,
    )
    .unwrap();
    let recorder = Arc::new(Recorder::default());
    let factory = ScopeFactory::with_parts(
        AgentActivityCapturePolicyV1 {
            capture: CaptureModeV1::Diagnostic,
            max_inline_bytes: 64,
            ..Default::default()
        },
        Arc::new(FakeClock::new(0..10)),
        vec![recorder.clone()],
    );
    let run = factory.main("main", ModelIdentity::new("p", "m"));
    run.observability.events.emit(AgentEvent::ToolEnd {
        id: "source-evidence".to_string(),
        name: "codebase_memory_get_code_snippet".to_string(),
        status: ToolCallStatus::Succeeded,
        duration_ms: 4,
        result: ToolResultMetadata {
            preview: Some(format!("provider source {SECRET}")),
            bytes: 128,
            truncated: false,
            failure: None,
            codebase_memory_timing: None,
            graph_correlation: Some(correlation),
            decision_anchor_lineage: Some(lineage.clone()),
        },
    });

    let frames = recorder.0.lock().expect("frames");
    let finished = frames
        .iter()
        .find_map(|frame| match &frame.event {
            AgentActivityEventV1::ToolFinished(value) => Some(value),
            _ => None,
        })
        .expect("source completion");
    assert_eq!(finished.decision_anchor_lineage.as_ref(), Some(&lineage));
    assert_eq!(finished.result, None);
    let activity = serde_json::to_string(&*frames).unwrap();
    assert!(activity.contains(r#""decision_evidence_kind":"implementation""#));
    assert!(!activity.contains(SECRET));
}

#[test]
fn ordinary_failures_keep_only_shell_owned_diagnostics_in_every_capture_mode() {
    const SECRET: &str = "Authorization: Bearer ORDINARY-FAILURE-SECRET";
    for mode in [
        CaptureModeV1::Metadata,
        CaptureModeV1::Transcript,
        CaptureModeV1::Diagnostic,
    ] {
        let recorder = Arc::new(Recorder::default());
        let factory = ScopeFactory::with_parts(
            AgentActivityCapturePolicyV1 {
                capture: mode,
                max_inline_bytes: 256,
                ..Default::default()
            },
            Arc::new(FakeClock::new(0..10)),
            vec![recorder.clone()],
        );
        let run = factory.main("main", ModelIdentity::new("p", "m"));
        let mut failure = ToolFailureDiagnostic::execution(ToolFailureReason::ToolReportedFailure);
        failure.message = format!("{SECRET} /private/path");
        run.observability.events.emit(AgentEvent::ToolEnd {
            id: "ordinary-failed".to_string(),
            name: "bash".to_string(),
            status: ToolCallStatus::Failed,
            duration_ms: 1,
            result: ToolResultMetadata {
                preview: Some(format!("stderr {SECRET}")),
                bytes: 100,
                truncated: false,
                failure: Some(failure),
                codebase_memory_timing: None,
                graph_correlation: None,
                decision_anchor_lineage: None,
            },
        });

        let frames = recorder.0.lock().expect("frames");
        let finished = frames
            .iter()
            .find_map(|frame| match &frame.event {
                AgentActivityEventV1::ToolFinished(value) => Some(value),
                _ => None,
            })
            .expect("tool finish");
        assert_eq!(finished.name, "bash");
        assert_eq!(finished.result, None);
        let diagnostic = finished.failure.as_ref().expect("typed failure");
        assert_eq!(diagnostic.category, ToolFailureCategoryV1::ExecutionFailure);
        assert_eq!(diagnostic.reason, ToolFailureReasonV1::ToolReportedFailure);
        assert_eq!(
            diagnostic.retry_disposition,
            ToolRetryDispositionV1::CorrectInvocation
        );
        let rendered = format!("{frames:?} {}", serde_json::to_string(&*frames).unwrap());
        assert!(!rendered.contains(SECRET));
        assert!(!rendered.contains("/private/path"));
    }
}

#[test]
fn actionable_graph_recovery_activity_retains_only_closed_missing_kinds_and_allowance() {
    const SECRET: &str = "Authorization: Bearer CLOSED-RECOVERY/src/private.rs --selector secret";
    let recorder = Arc::new(Recorder::default());
    let factory = ScopeFactory::with_parts(
        AgentActivityCapturePolicyV1 {
            capture: CaptureModeV1::Diagnostic,
            max_inline_bytes: 256,
            ..Default::default()
        },
        Arc::new(FakeClock::new(0..10)),
        vec![recorder.clone()],
    );
    let run = factory.main("main", ModelIdentity::new("p", "m"));
    let details = temper_protocol_activity::GraphExplorationClosedV1::recoverable(
        [
            temper_protocol_activity::GraphRecoveryEvidenceKindV1::Trace,
            temper_protocol_activity::GraphRecoveryEvidenceKindV1::Caller,
        ],
        2,
    )
    .unwrap();
    let mut failure = ToolFailureDiagnostic::graph_exploration(details.clone());
    failure.message = SECRET.to_string();
    run.observability.events.emit(AgentEvent::ToolEnd {
        id: "closed-recovery".to_string(),
        name: "codebase_memory_search_graph".to_string(),
        status: ToolCallStatus::Failed,
        duration_ms: 0,
        result: ToolResultMetadata {
            preview: Some(SECRET.to_string()),
            bytes: SECRET.len() as u64,
            truncated: false,
            failure: Some(failure),
            codebase_memory_timing: None,
            graph_correlation: None,
            decision_anchor_lineage: None,
        },
    });

    let frames = recorder.0.lock().expect("frames");
    let failure = frames
        .iter()
        .find_map(|frame| match &frame.event {
            AgentActivityEventV1::ToolFinished(finished) => finished.failure.as_ref(),
            _ => None,
        })
        .expect("safe recovery diagnostic");
    assert_eq!(
        failure.reason,
        ToolFailureReasonV1::DecisionEvidenceIncomplete
    );
    assert_eq!(failure.graph_exploration.as_ref(), Some(&details));
    let rendered = format!("{frames:?} {}", serde_json::to_string(&*frames).unwrap());
    assert!(!rendered.contains(SECRET));
    assert!(!rendered.contains("selector"));
}

#[test]
fn exploration_closed_activity_retains_only_the_stable_local_reason() {
    const SECRET: &str = "Authorization: Bearer CLOSED-GRAPH-SECRET/src/private.rs";
    let recorder = Arc::new(Recorder::default());
    let factory = ScopeFactory::with_parts(
        AgentActivityCapturePolicyV1 {
            capture: CaptureModeV1::Diagnostic,
            max_inline_bytes: 256,
            ..Default::default()
        },
        Arc::new(FakeClock::new(0..10)),
        vec![recorder.clone()],
    );
    let run = factory.main("main", ModelIdentity::new("p", "m"));
    let mut failure =
        ToolFailureDiagnostic::codebase_memory(ToolFailureCategory::GraphLifecycleDenial);
    failure.message = SECRET.to_string();
    run.observability.events.emit(AgentEvent::ToolEnd {
        id: "closed-graph".to_string(),
        name: "codebase_memory_search_graph".to_string(),
        status: ToolCallStatus::Failed,
        duration_ms: 0,
        result: ToolResultMetadata {
            preview: Some(SECRET.to_string()),
            bytes: SECRET.len() as u64,
            truncated: false,
            failure: Some(failure),
            codebase_memory_timing: None,
            graph_correlation: None,
            decision_anchor_lineage: None,
        },
    });

    let frames = recorder.0.lock().expect("frames");
    let finished = frames
        .iter()
        .find_map(|frame| match &frame.event {
            AgentActivityEventV1::ToolFinished(value) => Some(value),
            _ => None,
        })
        .expect("tool finish retained");
    let failure = finished.failure.as_ref().expect("safe local reason");
    assert_eq!(
        failure.category,
        ToolFailureCategoryV1::GraphLifecycleDenial
    );
    assert_eq!(failure.reason, ToolFailureReasonV1::ExplorationClosed);
    assert_eq!(failure.graph_exploration, None);
    assert_eq!(
        failure.message,
        "codebase-memory exploration is closed for this run; continue with conventional tools"
    );
    assert_eq!(finished.result, None);
    assert!(!serde_json::to_string(&*frames).unwrap().contains(SECRET));
}
