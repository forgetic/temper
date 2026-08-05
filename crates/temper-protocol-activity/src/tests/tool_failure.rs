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
    assert_eq!(serde_json::to_value(parsed).unwrap(), legacy);
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
    assert_code(event.validate(), ActivityValidationCode::InvalidEvent);
}
