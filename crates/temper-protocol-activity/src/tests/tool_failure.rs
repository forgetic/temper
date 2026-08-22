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
        (
            ToolFailureCategoryV1::GraphLifecycleDenial,
            "graph_lifecycle_denial",
        ),
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
fn graph_recovery_details_round_trip_with_sorted_kinds_and_no_private_inputs() {
    const SECRET: &str = "Authorization: Bearer RECOVERY-SECRET/src/private.rs";
    let details = GraphExplorationClosedV1::recoverable(
        [
            GraphRecoveryEvidenceKindV1::FocusedTest,
            GraphRecoveryEvidenceKindV1::Trace,
            GraphRecoveryEvidenceKindV1::Caller,
            GraphRecoveryEvidenceKindV1::Caller,
        ],
        3,
    )
    .expect("bounded recovery details");
    assert_eq!(
        details.missing_evidence,
        [
            GraphRecoveryEvidenceKindV1::Trace,
            GraphRecoveryEvidenceKindV1::Caller,
            GraphRecoveryEvidenceKindV1::FocusedTest,
        ]
    );
    let diagnostic = ToolFailureDiagnosticV1::with_graph_exploration(details.clone());
    assert_eq!(
        diagnostic.reason,
        ToolFailureReasonV1::DecisionEvidenceIncomplete
    );
    assert_eq!(
        diagnostic.retry_disposition,
        ToolRetryDispositionV1::CorrectInvocation
    );
    assert!(!diagnostic.fallback_to_conventional_discovery);
    assert!(diagnostic.message.contains("remaining allowance: 3"));
    assert!(diagnostic.message.contains("trace, caller, focused_test"));

    let encoded = serde_json::to_string(&diagnostic).unwrap();
    assert!(encoded.contains(r#""permitted_action":"targeted_current_root_graph_call""#));
    assert!(!encoded.contains(SECRET));
    assert_eq!(
        serde_json::from_str::<ToolFailureDiagnosticV1>(&encoded).unwrap(),
        diagnostic
    );
    let mut malformed: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    malformed["graph_exploration"]["missing_evidence"] =
        serde_json::json!(["focused_test", "trace"]);
    assert!(serde_json::from_value::<ToolFailureDiagnosticV1>(malformed).is_err());

    let mut event = usage_event(1);
    event.event = AgentActivityEventV1::ToolFinished(ToolFinishedV1 {
        call_id: "closed-recovery".into(),
        name: "codebase_memory_search_graph".into(),
        status: ToolStatusV1::Failed,
        duration_ms: 0,
        result: None,
        failure: Some(diagnostic),
        codebase_memory_timing: None,
        graph_correlation: None,
        decision_anchor_lineage: None,
    });
    event.validate().expect("closed recovery details validate");

    let mut forged = details;
    forged.missing_evidence.reverse();
    let mut forged_diagnostic =
        ToolFailureDiagnosticV1::with_graph_exploration(GraphExplorationClosedV1::completed());
    forged_diagnostic.reason = ToolFailureReasonV1::DecisionEvidenceIncomplete;
    forged_diagnostic.graph_exploration = Some(forged);
    let AgentActivityEventV1::ToolFinished(finished) = &mut event.event else {
        unreachable!();
    };
    finished.failure = Some(forged_diagnostic);
    assert_code(event.validate(), ActivityValidationCode::InvalidEvent);
    assert!(!format!("{event:?}").contains(SECRET));
}

#[test]
fn completed_and_exhausted_graph_closures_have_distinct_safe_actions() {
    let completed =
        ToolFailureDiagnosticV1::with_graph_exploration(GraphExplorationClosedV1::completed());
    assert_eq!(completed.reason, ToolFailureReasonV1::ExplorationClosed);
    assert_eq!(
        completed.graph_exploration.unwrap().permitted_action,
        GraphRecoveryPermittedActionV1::ConventionalDiscovery
    );

    let exhausted = ToolFailureDiagnosticV1::with_graph_exploration(
        GraphExplorationClosedV1::exhausted([GraphRecoveryEvidenceKindV1::Implementation]).unwrap(),
    );
    assert_eq!(
        exhausted.reason,
        ToolFailureReasonV1::DecisionEvidenceRecoveryExhausted
    );
    assert_eq!(
        exhausted.retry_disposition,
        ToolRetryDispositionV1::DoNotRetry
    );
    assert_eq!(
        exhausted.graph_exploration.unwrap().permitted_action,
        GraphRecoveryPermittedActionV1::StopWithoutProduct
    );
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
fn local_shell_denial_disposition_is_closed_versioned_and_privacy_safe() {
    const PRIVATE: [&str; 7] = [
        "DENIED-COMMAND",
        "PRIVATE-ARGV",
        "/private/path",
        "PROVIDER-VALUE",
        "PROMPT-VALUE",
        "CREDENTIAL-VALUE",
        "PROCESS-LOCAL-VALUE",
    ];
    let disposition = ShellDiscoveryDispositionV1::excluded_never_executed_local_policy_denial();
    assert!(disposition.is_valid());
    let encoded = serde_json::to_string(&disposition).unwrap();
    assert_eq!(
        encoded,
        r#"{"version":1,"status":"excluded_never_executed_local_policy_denial","matching_discovery_segments":0}"#
    );
    let debug = format!("{disposition:?}");
    for private in PRIVATE {
        assert!(!encoded.contains(private));
        assert!(!debug.contains(private));
    }

    let event: AgentActivityEventV1 = round_trip(&fixture("tool-started-local-denial.json"));
    let AgentActivityEventV1::ToolStarted(started) = &event else {
        panic!("local denial fixture must be a tool start");
    };
    assert_eq!(started.arguments, None);
    assert_eq!(started.shell_discovery_disposition, Some(disposition));
    let mut run_event = usage_event(1);
    run_event.event = event;
    run_event.validate().expect("closed denial validates");
}

#[test]
fn malformed_or_untrusted_shell_dispositions_fail_closed() {
    let canonical = serde_json::json!({
        "call_id": "denied-bash",
        "name": "bash",
        "shell_discovery_disposition": {
            "version": 1,
            "status": "excluded_never_executed_local_policy_denial",
            "matching_discovery_segments": 0
        }
    });
    let started: ToolStartedV1 = serde_json::from_value(canonical.clone()).unwrap();

    for malformed in [
        {
            let mut value = started.clone();
            value.name = "read".into();
            value
        },
        {
            let mut value = started.clone();
            value.shell_discovery_disposition.as_mut().unwrap().version = 2;
            value
        },
        {
            let mut value = started.clone();
            value
                .shell_discovery_disposition
                .as_mut()
                .unwrap()
                .matching_discovery_segments = 1;
            value
        },
        {
            let mut value = started.clone();
            value.arguments = Some(CapturedContentV1::Inline(InlineContentV1 {
                text: "DENIED-COMMAND".into(),
                truncated: false,
            }));
            value
        },
    ] {
        let mut event = usage_event(1);
        event.event = AgentActivityEventV1::ToolStarted(malformed);
        assert_code(event.validate(), ActivityValidationCode::InvalidEvent);
    }

    let mut unknown_status = canonical.clone();
    unknown_status["shell_discovery_disposition"]["status"] = serde_json::json!("executed");
    assert!(serde_json::from_value::<ToolStartedV1>(unknown_status).is_err());
    let mut unknown_field = canonical;
    unknown_field["shell_discovery_disposition"]["command"] = serde_json::json!("DENIED-COMMAND");
    assert!(serde_json::from_value::<ToolStartedV1>(unknown_field).is_err());
}

#[test]
fn legacy_tool_start_without_disposition_remains_readable() {
    let legacy = serde_json::json!({
        "call_id": "legacy-bash",
        "name": "bash"
    });
    let parsed: ToolStartedV1 = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(parsed.arguments, None);
    assert_eq!(parsed.shell_discovery_disposition, None);
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
        decision_evidence_kind: None,
    });
    assert_code(event.validate(), ActivityValidationCode::InvalidEvent);
    event.event.sanitize_graph_correlation();
    let AgentActivityEventV1::ToolFinished(finished) = &event.event else {
        unreachable!();
    };
    assert_eq!(finished.decision_anchor_lineage, None);
}

#[test]
fn decision_evidence_is_closed_source_only_and_privacy_safe() {
    const SECRET: &str = "Authorization: Bearer DECISION-EVIDENCE-SECRET";
    let source = GraphCorrelationV1::new(
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
        DecisionEvidenceKindV1::FocusedTest,
    )
    .unwrap();
    assert!(lineage.is_valid_for(&source));
    let encoded = serde_json::to_string(&lineage).unwrap();
    assert!(encoded.contains(r#""decision_evidence_kind":"focused_test""#));
    assert!(!encoded.contains(SECRET));

    let invalid_kind = serde_json::json!({
        "version": 1,
        "root_binding": "00000000-0000-4000-8000-000000000001",
        "stage": "carry_forward",
        "target_kind": "qualified_name",
        "decision_evidence_kind": "test-like provider prose"
    });
    assert!(serde_json::from_value::<DecisionAnchorLineageV1>(invalid_kind).is_err());
    assert!(
        DecisionAnchorLineageV1::new_with_decision_evidence_kind(
            "00000000-0000-4000-8000-000000000001".to_string(),
            DecisionAnchorLineageStageV1::Root,
            DecisionAnchorTargetKindV1::GraphQuery,
            [],
            DecisionEvidenceKindV1::Implementation,
        )
        .is_none(),
        "non-source lineage cannot manufacture semantic evidence"
    );

    let mut event = usage_event(1);
    event.event = AgentActivityEventV1::ToolFinished(ToolFinishedV1 {
        call_id: "source-1".into(),
        name: "codebase_memory_get_code_snippet".into(),
        status: ToolStatusV1::Succeeded,
        duration_ms: 5,
        result: None,
        failure: None,
        codebase_memory_timing: None,
        graph_correlation: Some(source),
        decision_anchor_lineage: Some(lineage),
    });
    event.validate().expect("closed source evidence validates");
    let activity = serde_json::to_string(&event).unwrap();
    assert!(activity.contains(r#""decision_evidence_kind":"focused_test""#));
    assert!(!activity.contains(SECRET));
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
