// SPDX-License-Identifier: MPL-2.0

//! Contract tests for the typed `validation.outcome` machine and human
//! projections.

use std::collections::BTreeSet;
use std::io;
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value};
use temper_log::emit::{self, ValidationOutcome, ValidationOutcomeKind};
use temper_log::{Event, WorkItemRef, validation_summary_preview};
use tracing::subscriber::with_default;
use tracing_subscriber::fmt::MakeWriter;

#[test]
fn validation_outcome_projects_every_safe_typed_field() {
    let record = capture("  Automated checks pass.\n Follow-up is required.  ");
    let fields = fields(&record);

    assert_eq!(
        record.get("target").and_then(Value::as_str),
        Some("temper::engine")
    );
    for (field, expected) in [
        ("service", "engine"),
        ("event", Event::ValidationOutcome.as_str()),
        ("repo", "acme/widgets"),
        ("artifact.ref", "acme/widgets#42"),
        ("outcome", "needs_followup"),
        ("role", "tester"),
        ("forge.actor.handle", "architect"),
        ("forge.actor.id", "forge-user-9"),
        ("job_id", "job-42"),
        ("transition", "plan_validation_needs_followup"),
        ("correlation.key", "validate-plan-42"),
        (
            "summary.preview",
            "Automated checks pass. Follow-up is required.",
        ),
    ] {
        assert_eq!(fields.get(field).and_then(Value::as_str), Some(expected));
    }
    assert_eq!(
        fields.get("validation.scope_count").and_then(Value::as_u64),
        Some(3)
    );
    assert_eq!(
        fields.get("follow_up.count").and_then(Value::as_u64),
        Some(2)
    );
    assert_eq!(
        fields.get("message").and_then(Value::as_str),
        Some(
            "engine:  [acme/widgets#42] validation outcome=needs_followup | role=tester actor=architect (forge-user-9) | job=job-42 transition=plan_validation_needs_followup correlation=validate-plan-42 | scope=3 follow-ups=2 | Automated checks pass. Follow-up is required."
        )
    );

    // The closed input and this exact projection contain no place for result
    // bodies, details, child bodies, tool output, or credentials.
    let actual_keys = fields.keys().map(String::as_str).collect::<BTreeSet<_>>();
    let expected_keys = [
        "artifact.ref",
        "correlation.key",
        "event",
        "follow_up.count",
        "forge.actor.handle",
        "forge.actor.id",
        "job_id",
        "message",
        "outcome",
        "repo",
        "role",
        "service",
        "summary.preview",
        "transition",
        "validation.scope_count",
    ]
    .into_iter()
    .collect();
    assert_eq!(actual_keys, expected_keys);
}

#[test]
fn folded_credentials_and_oversized_summaries_match_in_both_event_projections() {
    for (summary, secret) in [
        (
            "Checks pass, but Authorization: Bearer VALIDATION-AUTH-SENTINEL must not escape",
            "VALIDATION-AUTH-SENTINEL",
        ),
        (
            "Checks pass, but Bearer\tVALIDATION-BEARER-TAB-SENTINEL must not escape",
            "VALIDATION-BEARER-TAB-SENTINEL",
        ),
        (
            "Checks pass, but Bearer\nVALIDATION-BEARER-NEWLINE-SENTINEL must not escape",
            "VALIDATION-BEARER-NEWLINE-SENTINEL",
        ),
        (
            "Checks pass, but token \t=\n VALIDATION-TOKEN-SENTINEL must not escape",
            "VALIDATION-TOKEN-SENTINEL",
        ),
        (
            "Checks pass, but api_key\n:\tVALIDATION-API-KEY-SENTINEL must not escape",
            "VALIDATION-API-KEY-SENTINEL",
        ),
    ] {
        let secret_record = capture(summary);
        let secret_fields = fields(&secret_record);
        assert_eq!(
            secret_fields.get("summary.preview").and_then(Value::as_str),
            Some("<redacted>"),
            "credential shape was not redacted: {summary:?}"
        );
        let secret_message = secret_fields
            .get("message")
            .and_then(Value::as_str)
            .unwrap();
        assert!(secret_message.ends_with(" | <redacted>"));
        assert!(!secret_record.to_string().contains(secret));
    }

    let oversized = format!("  validation\n{}  ", "界".repeat(300));
    let expected = validation_summary_preview(&oversized);
    let oversized_record = capture(&oversized);
    let oversized_fields = fields(&oversized_record);
    assert_eq!(
        oversized_fields
            .get("summary.preview")
            .and_then(Value::as_str),
        Some(expected.as_str())
    );
    assert_eq!(expected.chars().count(), 240);
    assert!(expected.ends_with('…'));
    assert!(
        oversized_fields
            .get("message")
            .and_then(Value::as_str)
            .unwrap()
            .ends_with(&format!(" | {expected}"))
    );
}

fn capture(summary: &str) -> Value {
    let buffer = SharedBuffer::default();
    let sink = buffer.clone();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_writer(sink)
        .with_max_level(tracing::Level::TRACE)
        .finish();

    with_default(subscriber, || {
        let item = WorkItemRef::issue("acme/widgets", 42);
        emit::emit_validation_outcome(ValidationOutcome {
            item: &item,
            outcome: ValidationOutcomeKind::NeedsFollowup,
            workflow_role: "tester",
            forge_actor_handle: "architect",
            forge_actor_id: "forge-user-9",
            job_id: "job-42",
            transition: "plan_validation_needs_followup",
            correlation_key: "validate-plan-42",
            validation_scope_count: 3,
            follow_up_count: 2,
            summary,
        });
    });

    serde_json::from_slice(&buffer.bytes()).expect("validation event is one JSON record")
}

fn fields(record: &Value) -> &Map<String, Value> {
    record
        .get("fields")
        .and_then(Value::as_object)
        .expect("JSON event has structured fields")
}

#[derive(Clone, Default)]
struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

impl SharedBuffer {
    fn bytes(&self) -> Vec<u8> {
        self.0.lock().expect("capture lock").clone()
    }
}

impl io::Write for SharedBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .expect("capture lock")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for SharedBuffer {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}
