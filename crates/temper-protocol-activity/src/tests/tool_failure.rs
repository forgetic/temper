use super::*;

fn diagnostic(category: ToolFailureCategoryV1) -> ToolFailureDiagnosticV1 {
    ToolFailureDiagnosticV1::new(category)
}

#[test]
fn tool_failure_categories_have_stable_wire_names_and_safe_messages() {
    let cases = [
        (
            ToolFailureCategoryV1::ConfigurationStartup,
            "configuration_startup",
        ),
        (ToolFailureCategoryV1::ProjectNotReady, "project_not_ready"),
        (ToolFailureCategoryV1::IndexFailure, "index_failure"),
        (ToolFailureCategoryV1::Timeout, "timeout"),
        (ToolFailureCategoryV1::Transport, "transport"),
        (ToolFailureCategoryV1::ProcessExit, "process_exit"),
        (ToolFailureCategoryV1::ProviderProtocol, "provider_protocol"),
        (
            ToolFailureCategoryV1::InvalidModelInput,
            "invalid_model_input",
        ),
        (ToolFailureCategoryV1::CircuitOpen, "circuit_open"),
    ];

    for (category, wire_name) in cases {
        let value = serde_json::to_value(diagnostic(category)).unwrap();
        assert_eq!(value["category"], wire_name);
        assert_eq!(value["message"], category.safe_message());
        assert!(
            value["fallback_to_conventional_discovery"]
                .as_bool()
                .unwrap()
        );
    }
}

#[test]
fn tool_failure_wire_redacts_forged_and_oversized_messages_deterministically() {
    const SECRET: &str = "Authorization: Bearer TOOL-FAILURE-SECRET";
    let mut forged = diagnostic(ToolFailureCategoryV1::Timeout);
    forged.message = format!("{SECRET} {}", "x".repeat(10_000));
    forged.retryable = false;
    forged.fallback_to_conventional_discovery = false;

    let serialized = serde_json::to_string(&forged).unwrap();
    assert!(!serialized.contains(SECRET));
    assert!(!serialized.contains(&"x".repeat(1_000)));
    assert!(serialized.contains("codebase-memory request timed out"));
    assert!(serialized.contains(r#""retryable":true"#));
    assert!(serialized.contains(r#""fallback_to_conventional_discovery":true"#));

    let parsed: ToolFailureDiagnosticV1 = serde_json::from_value(serde_json::json!({
        "category": "process_exit",
        "retryable": true,
        "fallback_to_conventional_discovery": false,
        "message": format!("{SECRET} {}", "y".repeat(10_000))
    }))
    .unwrap();
    assert_eq!(
        parsed.message,
        ToolFailureCategoryV1::ProcessExit.safe_message()
    );
    assert!(!parsed.retryable);
    assert!(parsed.fallback_to_conventional_discovery);
    assert!(parsed.message.len() <= MAX_TOOL_FAILURE_MESSAGE_BYTES);
}

#[test]
fn old_tool_finished_events_omit_the_optional_failure() {
    let legacy = serde_json::json!({
        "call_id": "tool-legacy",
        "name": "read",
        "status": "failed",
        "duration_ms": 7
    });
    let parsed: ToolFinishedV1 = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(parsed.failure, None);
    assert_eq!(parsed.result, None);
    assert_eq!(parsed.codebase_memory_timing, None);
    assert_eq!(parsed.graph_correlation, None);
    assert_eq!(serde_json::to_value(parsed).unwrap(), legacy);
}

#[test]
fn graph_correlation_fingerprints_closed_targets_without_retaining_raw_arguments() {
    const SECRET: &str = "Authorization: Bearer GRAPH-CORRELATION-SECRET";
    let correlation = GraphCorrelationV1::new(
        GraphCorrelationToolV1::SearchGraph,
        GraphCorrelationTargetKindV1::GraphQuery,
        &format!("  activity   correlation {SECRET}  "),
    )
    .expect("complete graph query is fingerprinted");
    assert!(correlation.is_valid());
    assert_eq!(
        correlation.target_digest,
        GraphCorrelationV1::target_digest(&format!("activity correlation {SECRET}"))
            .expect("normalized digest")
    );
    assert!(
        GraphCorrelationV1::new(
            GraphCorrelationToolV1::SearchGraph,
            GraphCorrelationTargetKindV1::Pattern,
            "unsupported pair",
        )
        .is_none()
    );
    assert!(GraphCorrelationV1::target_digest("bad\ncontrol").is_none());
    assert!(
        GraphCorrelationV1::target_digest(&"x".repeat(MAX_GRAPH_CORRELATION_TARGET_BYTES + 1))
            .is_none()
    );

    let rendered = serde_json::to_string(&correlation).expect("correlation serializes");
    assert!(!rendered.contains(SECRET));
    assert!(!rendered.contains("activity correlation"));
    assert!(rendered.contains(&correlation.target_digest));

    let mut event = usage_event(1);
    event.event = AgentActivityEventV1::ToolFinished(ToolFinishedV1 {
        call_id: "graph-1".into(),
        name: "codebase_memory_search_graph".into(),
        status: ToolStatusV1::Succeeded,
        duration_ms: 5,
        result: None,
        failure: None,
        codebase_memory_timing: None,
        graph_correlation: Some(correlation),
    });
    event.validate().expect("closed correlation validates");
    let export = TraceExportRecordV1::event(event.clone());
    let export_json = serde_json::to_string(&export).expect("export serializes");
    assert!(!export_json.contains(SECRET));
    assert_eq!(
        serde_json::from_str::<TraceExportRecordV1>(&export_json).expect("export parses"),
        export
    );

    let AgentActivityEventV1::ToolFinished(finished) = &mut event.event else {
        unreachable!();
    };
    finished.graph_correlation.as_mut().unwrap().target_digest = SECRET.to_string();
    assert_code(event.validate(), ActivityValidationCode::InvalidEvent);
    event.event.sanitize_graph_correlation();
    let AgentActivityEventV1::ToolFinished(finished) = &event.event else {
        unreachable!();
    };
    assert_eq!(finished.graph_correlation, None);
}

#[test]
fn tool_failures_validate_only_on_non_success_boundaries() {
    let mut event = usage_event(1);
    event.event = AgentActivityEventV1::ToolFinished(ToolFinishedV1 {
        call_id: "graph-1".into(),
        name: "codebase_memory_search_graph".into(),
        status: ToolStatusV1::Failed,
        duration_ms: 50,
        result: None,
        failure: Some(diagnostic(ToolFailureCategoryV1::Timeout)),
        codebase_memory_timing: Some(CodebaseMemoryTimingV1 {
            readiness_wait_ms: 10,
            graph_execution_ms: 40,
        }),
        graph_correlation: None,
    });
    event.validate().expect("failed tool diagnostic validates");

    let AgentActivityEventV1::ToolFinished(finished) = &mut event.event else {
        unreachable!();
    };
    let failure = finished.failure.as_mut().expect("diagnostic");
    failure.retryable = false;
    assert_code(event.validate(), ActivityValidationCode::InvalidEvent);

    let AgentActivityEventV1::ToolFinished(finished) = &mut event.event else {
        unreachable!();
    };
    finished.failure.as_mut().expect("diagnostic").normalize();
    finished.status = ToolStatusV1::Succeeded;
    assert_code(event.validate(), ActivityValidationCode::InvalidEvent);

    let AgentActivityEventV1::ToolFinished(finished) = &mut event.event else {
        unreachable!();
    };
    finished.status = ToolStatusV1::Failed;
    finished.name = "bash".into();
    finished.failure = None;
    finished
        .codebase_memory_timing
        .as_mut()
        .unwrap()
        .graph_execution_ms = 41;
    assert_code(event.validate(), ActivityValidationCode::InvalidEvent);
}
