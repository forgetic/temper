use std::sync::{Arc, Mutex};

use tracing::Level;
use tracing::field::{Field, Visit};
use tracing::subscriber::with_default;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry;

use super::*;

fn fixture_request() -> WorkflowRoleDecisionRequest {
    serde_json::from_str(include_str!(
        "../../temper-protocol-decision/fixtures/workflow-role-decision-request.json"
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

/// One captured tracing event: level, target, the human `message` projection,
/// and the string-valued fields the assertions key off.
#[derive(Clone, Debug)]
struct Captured {
    level: Level,
    target: String,
    message: Option<String>,
    fields: std::collections::BTreeMap<String, String>,
}

impl Captured {
    fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(String::as_str)
    }
}

#[derive(Default)]
struct Visitor {
    message: Option<String>,
    fields: std::collections::BTreeMap<String, String>,
}

impl Visit for Visitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let rendered = format!("{value:?}");
        if field.name() == "message" {
            self.message = Some(rendered);
        } else {
            // Debug-formatted strings/arrays arrive quoted/bracketed; keep the
            // raw debug text — assertions use `contains`, so quoting is fine.
            self.fields.insert(field.name().to_string(), rendered);
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = Some(value.to_string());
        } else {
            self.fields
                .insert(field.name().to_string(), value.to_string());
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }
}

#[derive(Clone, Default)]
struct CaptureLayer {
    events: Arc<Mutex<Vec<Captured>>>,
}

impl<S: tracing::Subscriber> Layer<S> for CaptureLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = Visitor::default();
        event.record(&mut visitor);
        let metadata = event.metadata();
        self.events.lock().unwrap().push(Captured {
            level: *metadata.level(),
            target: metadata.target().to_string(),
            message: visitor.message,
            fields: visitor.fields,
        });
    }
}

/// Runs `body` under a capturing subscriber (no level filter, so debug/trace are
/// captured too) and returns every recorded event.
fn capture(body: impl FnOnce()) -> Vec<Captured> {
    let layer = CaptureLayer::default();
    let events = layer.events.clone();
    let subscriber = registry().with(layer);
    with_default(subscriber, body);
    Arc::try_unwrap(events)
        .expect("no outstanding layer references")
        .into_inner()
        .expect("capture mutex not poisoned")
}

/// The single event the supplied emitter produced. Each `emit_*` fn emits
/// exactly one line, so this both finds it and asserts that uniqueness.
fn single(events: &[Captured]) -> &Captured {
    assert_eq!(
        events.len(),
        1,
        "expected exactly one emitted event, got {events:?}"
    );
    &events[0]
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

/// Every `emit_*` line lands on the `temper::agent` namespace target with a
/// human message that is NOT raw JSON — the core §8 invariant for this path.
fn assert_clean_agent_line(event: &Captured) {
    assert_eq!(
        event.target, AGENT_TARGET,
        "decision line must ride the temper::agent namespace target, not temper_agent",
    );
    let message = event
        .message
        .as_deref()
        .expect("decision line has a human message");
    assert!(
        !message.trim_start().starts_with('{'),
        "decision line renders raw JSON in its message: {message:?}",
    );
    assert!(
        message.starts_with("agent:"),
        "decision line lacks the agent human projection: {message:?}",
    );
}

#[test]
fn request_event_logs_counts_identity_and_not_raw_bodies_or_credentials() {
    let mut request = fixture_request();
    request.work_item_context["artifact"]["body"] =
        serde_json::json!("THIS_BODY_MUST_NOT_APPEAR_IN_LOGS");
    let trace = WorkflowRoleTrace::from_work_item_context(&request.work_item_context);

    let events = capture(|| emit_request(&request, &trace, &provider(), 123, 456));
    let event = single(&events);

    assert_clean_agent_line(event);
    assert_eq!(event.level, Level::DEBUG);
    assert_eq!(
        event.field("event"),
        Some("anvil.workflow_role_decision.request")
    );
    assert_eq!(event.field("allowed_action_count"), Some("2"));
    assert_eq!(event.field("available_external_tool_count"), Some("1"));
    assert_eq!(event.field("auth_mode"), Some("api_key"));
    assert_eq!(event.field("provider"), Some("deepseek"));
    assert_eq!(event.field("model"), Some("deepseek-chat"));
    assert_eq!(event.field("prompt_chars"), Some("123"));
    assert_eq!(event.field("context_chars"), Some("456"));
    // The action/tool names render in the `?`-formatted array fields.
    let actions = event.field("allowed_actions").unwrap_or_default();
    assert!(actions.contains("no_action") && actions.contains("advance"));
    let tools = event.field("available_external_tools").unwrap_or_default();
    assert!(tools.contains("coding_workspace"));

    // No raw issue body and no credential anywhere in the captured event.
    let blob = format!("{event:?}");
    assert!(!blob.contains("THIS_BODY_MUST_NOT_APPEAR_IN_LOGS"));
    assert!(!blob.contains("sk-secret-do-not-log"));
}

#[test]
fn provider_finish_event_logs_error_classes_without_error_payloads() {
    let request = fixture_request();
    let trace = WorkflowRoleTrace::from_work_item_context(&request.work_item_context);
    let parse_error = DecisionError::Parse {
        snippet: "RAW_MODEL_PAYLOAD_SHOULD_NOT_LOG".to_string(),
        error: "expected value".to_string(),
    };

    let events = capture(|| {
        emit_provider_call_finish(
            &request,
            &trace,
            &provider(),
            17,
            ProviderCallLogOutcome::Error(&parse_error),
        )
    });
    let event = single(&events);
    assert_clean_agent_line(event);
    assert_eq!(event.level, Level::DEBUG);
    assert_eq!(event.field("outcome"), Some("error"));
    assert_eq!(event.field("parse_error_class"), Some("json_parse"));
    // Error variant leaves provider_error_class blank, not the model payload.
    assert_eq!(event.field("provider_error_class"), Some(""));
    let blob = format!("{event:?}");
    assert!(!blob.contains("RAW_MODEL_PAYLOAD_SHOULD_NOT_LOG"));
    assert!(!blob.contains("expected value"));

    let provider_error = DecisionError::Run("HTTP 429 RAW_PROVIDER_BODY".to_string());
    let events = capture(|| {
        emit_provider_call_finish(
            &request,
            &trace,
            &provider(),
            19,
            ProviderCallLogOutcome::Error(&provider_error),
        )
    });
    let event = single(&events);
    assert_eq!(event.field("provider_error_class"), Some("provider_run"));
    assert_eq!(event.field("parse_error_class"), Some(""));
    let blob = format!("{event:?}");
    assert!(!blob.contains("RAW_PROVIDER_BODY"));
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

    let events = capture(|| emit_reply(&request, &trace, &provider(), &reply, &metadata));
    let event = single(&events);

    assert_clean_agent_line(event);
    assert_eq!(event.level, Level::DEBUG);
    assert_eq!(
        event.field("outcome"),
        Some("unauthorized_action_downgraded")
    );
    assert_eq!(event.field("model_action"), Some("delete_everything"));
    assert_eq!(
        event.field("returned_action"),
        Some(WORKFLOW_ROLE_DECISION_NO_ACTION)
    );
    assert_eq!(event.field("unauthorized_action_downgraded"), Some("true"));
    assert_eq!(
        event.field("unauthorized_model_action"),
        Some("delete_everything")
    );
    assert!(event.field("reason_preview").unwrap().ends_with('…'));
    let blob = format!("{event:?}");
    assert!(!blob.contains("sk-secret-do-not-log"));
    assert!(!blob.contains("TAIL"));
}

/// Regression guard mirroring `tests/no_json_in_message.rs` (piece E) for the
/// decision-observability emitter, which that integration test cannot reach
/// (the `emit_*` fns are `pub(crate)`). Drives every decision line and asserts
/// that NONE of them land at `info` and that NO agent-target message is raw
/// JSON — so neither this emitter nor the `UsageLogger` can regress the §8
/// invariant.
#[test]
fn no_decision_line_is_info_level_json_in_message() {
    let request = fixture_request();
    let trace = WorkflowRoleTrace::from_work_item_context(&request.work_item_context);
    let p = provider();
    let reply = WorkflowRoleDecisionReply {
        protocol_version: request.protocol_version,
        action: WORKFLOW_ROLE_DECISION_NO_ACTION.to_string(),
        reason: "n/a".to_string(),
    };
    let metadata = ReplyLogMetadata::authorized_action("advance".to_string());
    let parse_error = DecisionError::Parse {
        snippet: "x".to_string(),
        error: "y".to_string(),
    };

    let events = capture(|| {
        emit_request(&request, &trace, &p, 10, 20);
        emit_provider_call_start(&request, &trace, &p);
        emit_provider_call_finish(
            &request,
            &trace,
            &p,
            5,
            ProviderCallLogOutcome::Model { action: "advance" },
        );
        emit_provider_call_finish(
            &request,
            &trace,
            &p,
            5,
            ProviderCallLogOutcome::Error(&parse_error),
        );
        emit_reply(&request, &trace, &p, &reply, &metadata);
        emit_capture_written(&request, &trace, &p, std::path::Path::new("/tmp/c.json"));
        emit_capture_write_failed(&request, &trace, &p, "permission_denied", "boom");
    });

    assert!(!events.is_empty(), "expected decision lines to be emitted");
    for event in &events {
        assert_eq!(
            event.target, AGENT_TARGET,
            "decision line on wrong target: {event:?}",
        );
        // §5: every decision line is a between-state cause -> debug, except the
        // failed-capture warning. NONE may be info.
        assert_ne!(
            event.level,
            Level::INFO,
            "decision line leaked at info: {event:?}",
        );
        if let Some(message) = &event.message {
            assert!(
                !message.trim_start().starts_with('{'),
                "decision line renders raw JSON-in-message: {message:?}",
            );
        }
    }
}

#[test]
fn capture_events_record_path_or_bounded_warning() {
    let request = fixture_request();
    let trace = WorkflowRoleTrace::from_work_item_context(&request.work_item_context);

    let events = capture(|| {
        emit_capture_written(
            &request,
            &trace,
            &provider(),
            std::path::Path::new("/tmp/anvil-captures/decision-1.json"),
        )
    });
    let event = single(&events);
    assert_clean_agent_line(event);
    assert_eq!(event.level, Level::DEBUG);
    assert_eq!(
        event.field("event"),
        Some("anvil.workflow_role_decision.capture.written")
    );
    assert_eq!(
        event.field("capture_path"),
        Some("/tmp/anvil-captures/decision-1.json")
    );

    let events = capture(|| {
        emit_capture_write_failed(
            &request,
            &trace,
            &provider(),
            "permission_denied",
            &format!(
                "password=hunter2 {}TAIL",
                "x".repeat(FIELD_PREVIEW_CHARS + 20)
            ),
        )
    });
    let event = single(&events);
    // The failed-capture line is the one decision event at warn, not debug.
    assert_eq!(event.level, Level::WARN);
    assert_clean_agent_line(event);
    assert_eq!(
        event.field("event"),
        Some("anvil.workflow_role_decision.capture.write_failed")
    );
    assert_eq!(event.field("outcome"), Some("warning"));
    assert_eq!(
        event.field("capture_error_class"),
        Some("permission_denied")
    );
    assert!(event.field("capture_error_preview").unwrap().ends_with('…'));
    let blob = format!("{event:?}");
    assert!(!blob.contains("hunter2"));
    assert!(!blob.contains("TAIL"));
}
