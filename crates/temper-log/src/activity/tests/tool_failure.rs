use super::*;

#[test]
fn failed_tool_span_projects_closed_diagnostic_without_raw_result_values() {
    const SECRET: &str = "Authorization: Bearer SPAN-TOOL-SECRET";
    let exporter = Arc::new(InMemoryActivitySpanExporter::default());
    let mut projector = CanonicalActivityProjector::new(exporter.clone());
    let main = scope("main-1", AgentScopeKindV1::Main, None);
    let mut diagnostic = ToolFailureDiagnosticV1::with_reason(
        ToolFailureCategoryV1::ExecutionFailure,
        ToolFailureReasonV1::ToolReportedFailure,
    );
    diagnostic.message = SECRET.to_string();
    diagnostic.normalize();
    projector.project_all(&[
        event(
            1,
            0,
            main.clone(),
            Some(0),
            Event::ToolStarted(ToolStartedV1 {
                call_id: "tool-failed".into(),
                name: "bash".into(),
                arguments: None,
            }),
        ),
        event(
            2,
            5,
            main,
            Some(0),
            Event::ToolFinished(ToolFinishedV1 {
                call_id: "tool-failed".into(),
                name: "bash".into(),
                status: ToolStatusV1::Failed,
                duration_ms: 5,
                result: None,
                failure: Some(diagnostic.clone()),
                codebase_memory_timing: None,
                graph_correlation: None,
                decision_anchor_lineage: None,
            }),
        ),
    ]);

    let spans = exporter.finished_spans();
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].start.kind, ActivitySpanKind::Tool);
    assert_eq!(spans[0].status, ActivitySpanStatus::Error);
    assert_eq!(spans[0].attributes.tool_failure, Some(diagnostic));
    assert!(!format!("{spans:?}").contains(SECRET));
}
