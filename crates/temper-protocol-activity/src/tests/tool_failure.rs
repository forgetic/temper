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
        assert_eq!(value["reason"], category.default_reason().as_str());
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
    assert!(!format!("{forged:?} {parsed:?}").contains(SECRET));
}

#[test]
fn ordinary_failure_reasons_and_dispositions_round_trip_canonically() {
    let cases = [
        (
            ToolFailureCategoryV1::SchemaArgumentMismatch,
            ToolFailureReasonV1::InvalidArguments,
            ToolRetryDispositionV1::CorrectInvocation,
        ),
        (
            ToolFailureCategoryV1::PolicyDenial,
            ToolFailureReasonV1::PolicyPrecondition,
            ToolRetryDispositionV1::SatisfyPolicy,
        ),
        (
            ToolFailureCategoryV1::ExecutionFailure,
            ToolFailureReasonV1::ToolReportedFailure,
            ToolRetryDispositionV1::CorrectInvocation,
        ),
        (
            ToolFailureCategoryV1::Timeout,
            ToolFailureReasonV1::DeadlineExceeded,
            ToolRetryDispositionV1::Retryable,
        ),
        (
            ToolFailureCategoryV1::Cancellation,
            ToolFailureReasonV1::RunCancelled,
            ToolRetryDispositionV1::DoNotRetry,
        ),
        (
            ToolFailureCategoryV1::GraphLifecycleDenial,
            ToolFailureReasonV1::ExplorationClosed,
            ToolRetryDispositionV1::ConventionalDiscovery,
        ),
        (
            ToolFailureCategoryV1::CircuitRedirect,
            ToolFailureReasonV1::RepeatedNonRetryable,
            ToolRetryDispositionV1::CorrectInvocation,
        ),
        (
            ToolFailureCategoryV1::CircuitRedirect,
            ToolFailureReasonV1::RetryBudgetExhausted,
            ToolRetryDispositionV1::CorrectInvocation,
        ),
    ];

    for (category, reason, disposition) in cases {
        let diagnostic = ToolFailureDiagnosticV1::with_reason(category, reason);
        assert_eq!(diagnostic.retry_disposition, disposition);
        let json = serde_json::to_string(&diagnostic).unwrap();
        assert!(json.contains(reason.as_str()));
        assert!(json.contains(disposition.as_str()));
        let decoded: ToolFailureDiagnosticV1 = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, diagnostic);
    }
}

#[test]
fn legacy_graph_diagnostic_without_reason_or_disposition_remains_readable() {
    let legacy = serde_json::json!({
        "category": "timeout",
        "retryable": false,
        "fallback_to_conventional_discovery": false,
        "message": "untrusted retained value"
    });
    let decoded: ToolFailureDiagnosticV1 = serde_json::from_value(legacy).unwrap();
    assert_eq!(decoded.category, ToolFailureCategoryV1::Timeout);
    assert_eq!(decoded.reason, ToolFailureReasonV1::GraphTimeout);
    assert_eq!(decoded.retry_disposition, ToolRetryDispositionV1::Retryable);
    assert!(decoded.retryable);
    assert_eq!(decoded.message, "codebase-memory request timed out");
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
        decision_anchor_lineage: None,
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
fn malformed_or_unbound_lineage_is_rejected_and_sanitized() {
    let correlation = GraphCorrelationV1::new(
        GraphCorrelationToolV1::SearchGraph,
        GraphCorrelationTargetKindV1::GraphQuery,
        "declared target",
    )
    .unwrap();
    let root = "00000000-0000-4000-8000-000000000001".to_string();
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
        decision_anchor_lineage: DecisionAnchorLineageV1::new_with_canonical_target_digests(
            root,
            DecisionAnchorLineageStageV1::Root,
            DecisionAnchorTargetKindV1::GraphQuery,
            [DecisionAnchorTargetKindV1::Pattern],
            [GraphCorrelationV1::target_digest("forged-root").unwrap()],
        ),
    });
    assert_eq!(
        event.validate(),
        Ok(()),
        "the constructor rejects root canonical evidence before an event is formed"
    );

    let AgentActivityEventV1::ToolFinished(finished) = &mut event.event else {
        unreachable!();
    };
    finished.decision_anchor_lineage = Some(DecisionAnchorLineageV1 {
        version: 1,
        root_binding: "00000000-0000-4000-8000-000000000001".to_string(),
        stage: DecisionAnchorLineageStageV1::Root,
        target_kind: DecisionAnchorTargetKindV1::GraphQuery,
        result_target_kinds: vec![DecisionAnchorTargetKindV1::Pattern],
        canonical_target_digests: vec![GraphCorrelationV1::target_digest("forged-root").unwrap()],
    });
    assert_code(event.validate(), ActivityValidationCode::InvalidEvent);
    event.event.sanitize_graph_correlation();
    let AgentActivityEventV1::ToolFinished(finished) = &event.event else {
        unreachable!();
    };
    assert_eq!(finished.decision_anchor_lineage, None);
}
#[test]
fn ordinary_tool_failures_validate_without_result_content() {
    let mut event = usage_event(1);
    event.event = AgentActivityEventV1::ToolFinished(ToolFinishedV1 {
        call_id: "ordinary-1".into(),
        name: "bash".into(),
        status: ToolStatusV1::Failed,
        duration_ms: 2,
        result: None,
        failure: Some(ToolFailureDiagnosticV1::with_reason(
            ToolFailureCategoryV1::ExecutionFailure,
            ToolFailureReasonV1::ToolReportedFailure,
        )),
        codebase_memory_timing: None,
        graph_correlation: None,
        decision_anchor_lineage: None,
    });
    event.validate().expect("ordinary typed failure validates");

    let AgentActivityEventV1::ToolFinished(finished) = &mut event.event else {
        unreachable!();
    };
    finished.result = Some(CapturedContentV1::Inline(InlineContentV1 {
        text: "Authorization: Bearer RESULT-SECRET".into(),
        truncated: false,
    }));
    assert_code(event.validate(), ActivityValidationCode::InvalidEvent);
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
        decision_anchor_lineage: None,
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
