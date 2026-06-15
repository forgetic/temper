use super::*;

fn fixture_request() -> WorkflowRoleDecisionRequest {
    serde_json::from_str(include_str!(
        "../../temper-process-protocol/fixtures/workflow-role-decision-request.json"
    ))
    .expect("Temper workflow-role decision fixture parses")
}

fn provider() -> ProviderConfig {
    ProviderConfig::new(
        "deepseek",
        "deepseek-chat",
        "https://api.example.invalid/v1",
        "sk-secret-do-not-log",
    )
}

#[test]
fn trace_extraction_reads_authority_neutral_fields() {
    let context = serde_json::json!({
        "observability": {
            "run_id": "run-1",
            "tick_id": "tick-1",
            "work_item_id": "work-item-1",
            "decision_id": "decision-1"
        },
        "repository": "forgejo:acme/service",
        "role": "architect",
        "queue": "intake",
        "kind": "epic",
        "artifact": {"type": "issue", "number": 42}
    });

    let trace = WorkflowRoleTrace::from_work_item_context(&context);

    assert_eq!(trace.run_id.as_deref(), Some("run-1"));
    assert_eq!(trace.tick_id.as_deref(), Some("tick-1"));
    assert_eq!(trace.work_item_id.as_deref(), Some("work-item-1"));
    assert_eq!(trace.decision_id.as_deref(), Some("decision-1"));
    assert_eq!(trace.repository.as_deref(), Some("forgejo:acme/service"));
    assert_eq!(trace.role.as_deref(), Some("architect"));
    assert_eq!(trace.queue.as_deref(), Some("intake"));
    assert_eq!(trace.kind.as_deref(), Some("epic"));
    assert_eq!(trace.artifact_type.as_deref(), Some("issue"));
    assert_eq!(trace.artifact_number.as_deref(), Some("42"));
}

#[test]
fn trace_extraction_tolerates_missing_or_non_scalar_fields() {
    let context = serde_json::json!({
        "observability": {"work_item_id": ["not", "scalar"]},
        "repository": {"nested": true},
        "artifact": {"number": null}
    });

    let trace = WorkflowRoleTrace::from_work_item_context(&context);

    assert_eq!(trace, WorkflowRoleTrace::default());
}

#[test]
fn trace_extraction_falls_back_to_observability_work_item_fields() {
    let context = serde_json::json!({
        "observability": {
            "repo": "forgejo:acme/service",
            "role": "builder",
            "queue": "todo",
            "artifact_kind": "task",
            "artifact_type": "issue",
            "artifact_number": 42
        }
    });

    let trace = WorkflowRoleTrace::from_work_item_context(&context);

    assert_eq!(trace.repository.as_deref(), Some("forgejo:acme/service"));
    assert_eq!(trace.role.as_deref(), Some("builder"));
    assert_eq!(trace.queue.as_deref(), Some("todo"));
    assert_eq!(trace.kind.as_deref(), Some("task"));
    assert_eq!(trace.artifact_type.as_deref(), Some("issue"));
    assert_eq!(trace.artifact_number.as_deref(), Some("42"));
}

#[test]
fn request_event_logs_counts_identity_and_not_raw_bodies_or_credentials() {
    let mut request = fixture_request();
    request.work_item_context["artifact"]["body"] =
        serde_json::json!("THIS_BODY_MUST_NOT_APPEAR_IN_LOGS");
    let trace = WorkflowRoleTrace::from_work_item_context(&request.work_item_context);

    let rendered = request_event(&request, &trace, &provider(), 123, 456).render();
    let parsed: Value = serde_json::from_str(&rendered).expect("event parses");

    assert_eq!(parsed["event"], "anvil.workflow_role_decision.request");
    assert_eq!(parsed["allowed_action_count"], 2);
    assert_eq!(
        parsed["allowed_actions"],
        serde_json::json!(["no_action", "advance"])
    );
    assert_eq!(parsed["available_external_tool_count"], 1);
    assert_eq!(
        parsed["available_external_tools"],
        serde_json::json!(["coding_workspace"])
    );
    assert_eq!(parsed["auth_mode"], "api_key");
    assert_eq!(parsed["provider"], "deepseek");
    assert_eq!(parsed["model"], "deepseek-chat");
    assert_eq!(parsed["prompt_chars"], 123);
    assert_eq!(parsed["context_chars"], 456);
    assert!(!rendered.contains("THIS_BODY_MUST_NOT_APPEAR_IN_LOGS"));
    assert!(!rendered.contains("sk-secret-do-not-log"));
}

#[test]
fn provider_finish_event_logs_error_classes_without_error_payloads() {
    let request = fixture_request();
    let trace = WorkflowRoleTrace::from_work_item_context(&request.work_item_context);
    let parse_error = DecisionError::Parse {
        snippet: "RAW_MODEL_PAYLOAD_SHOULD_NOT_LOG".to_string(),
        error: "expected value".to_string(),
    };

    let rendered = provider_call_finish_event(
        &request,
        &trace,
        &provider(),
        17,
        ProviderCallLogOutcome::Error(&parse_error),
    )
    .render();
    let parsed: Value = serde_json::from_str(&rendered).expect("event parses");

    assert_eq!(parsed["outcome"], "error");
    assert_eq!(parsed["parse_error_class"], "json_parse");
    assert!(parsed.get("provider_error_class").is_none());
    assert!(!rendered.contains("RAW_MODEL_PAYLOAD_SHOULD_NOT_LOG"));
    assert!(!rendered.contains("expected value"));

    let provider_error = DecisionError::Run("HTTP 429 RAW_PROVIDER_BODY".to_string());
    let rendered = provider_call_finish_event(
        &request,
        &trace,
        &provider(),
        19,
        ProviderCallLogOutcome::Error(&provider_error),
    )
    .render();
    let parsed: Value = serde_json::from_str(&rendered).expect("event parses");
    assert_eq!(parsed["provider_error_class"], "provider_run");
    assert!(parsed.get("parse_error_class").is_none());
    assert!(!rendered.contains("RAW_PROVIDER_BODY"));
}

#[test]
fn reply_event_records_unauthorized_downgrade_and_redacts_truncated_reason() {
    let request = fixture_request();
    let trace = WorkflowRoleTrace::from_work_item_context(&request.work_item_context);
    let long_reason = format!(
        "Bearer sk-secret-do-not-log {}TAIL",
        "x".repeat(REASON_PREVIEW_CHARS + 20)
    );
    let reply = WorkflowRoleDecisionReply {
        protocol_version: request.protocol_version,
        action: WORKFLOW_ROLE_DECISION_NO_ACTION.to_string(),
        reason: long_reason,
    };
    let metadata =
        ReplyLogMetadata::unauthorized_action_downgraded("delete_everything".to_string());

    let rendered = reply_event(&request, &trace, &provider(), &reply, &metadata).render();
    let parsed: Value = serde_json::from_str(&rendered).expect("event parses");

    assert_eq!(parsed["outcome"], "unauthorized_action_downgraded");
    assert_eq!(parsed["model_action"], "delete_everything");
    assert_eq!(parsed["returned_action"], WORKFLOW_ROLE_DECISION_NO_ACTION);
    assert_eq!(parsed["unauthorized_action_downgraded"], true);
    assert_eq!(parsed["unauthorized_model_action"], "delete_everything");
    assert!(parsed["reason_preview"].as_str().unwrap().ends_with('…'));
    assert!(!rendered.contains("sk-secret-do-not-log"));
    assert!(!rendered.contains("TAIL"));
}

#[test]
fn capture_events_record_path_or_bounded_warning() {
    let request = fixture_request();
    let trace = WorkflowRoleTrace::from_work_item_context(&request.work_item_context);

    let rendered = capture_written_event(
        &request,
        &trace,
        &provider(),
        std::path::Path::new("/tmp/anvil-captures/decision-1.json"),
    )
    .render();
    let parsed: Value = serde_json::from_str(&rendered).expect("event parses");
    assert_eq!(
        parsed["event"],
        "anvil.workflow_role_decision.capture.written"
    );
    assert_eq!(
        parsed["capture_path"],
        "/tmp/anvil-captures/decision-1.json"
    );

    let rendered = capture_write_failed_event(
        &request,
        &trace,
        &provider(),
        "permission_denied",
        &format!(
            "password=hunter2 {}TAIL",
            "x".repeat(FIELD_PREVIEW_CHARS + 20)
        ),
    )
    .render();
    let parsed: Value = serde_json::from_str(&rendered).expect("event parses");
    assert_eq!(
        parsed["event"],
        "anvil.workflow_role_decision.capture.write_failed"
    );
    assert_eq!(parsed["outcome"], "warning");
    assert_eq!(parsed["capture_error_class"], "permission_denied");
    assert!(
        parsed["capture_error_preview"]
            .as_str()
            .unwrap()
            .ends_with('…')
    );
    assert!(!rendered.contains("hunter2"));
    assert!(!rendered.contains("TAIL"));
}
