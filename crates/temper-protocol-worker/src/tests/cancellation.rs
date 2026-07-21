// SPDX-License-Identifier: MPL-2.0

use crate::{
    AttemptCancellation, AttemptCancellationCause, CancelAttempts,
    MAX_ATTEMPT_CANCELLATION_REASON_BYTES, WorkerProtocolMessage,
};

fn cancellation(job_id: &str, attempt_id: &str, reason: &str) -> AttemptCancellation {
    AttemptCancellation::ownership_lost("worker-a", job_id, attempt_id, reason)
        .expect("valid cancellation")
}

#[test]
fn multi_attempt_cancellation_round_trips_in_deterministic_order() {
    let directive = CancelAttempts::new(
        "worker-a",
        vec![
            cancellation("job-z", "attempt-2", "durable assignment was removed"),
            cancellation("job-a", "attempt-9", "durable assignment was replaced"),
            cancellation("job-a", "attempt-1", "artifact is no longer assigned"),
        ],
    )
    .expect("valid directive");
    let message = WorkerProtocolMessage::CancelAttempts(directive.clone());
    let json = serde_json::to_value(&message).expect("serializes");
    assert_eq!(json["type"], "cancel-attempts");
    assert_eq!(json["cancellations"][0]["job_id"], "job-a");
    assert_eq!(json["cancellations"][0]["attempt_id"], "attempt-1");
    assert_eq!(json["cancellations"][1]["attempt_id"], "attempt-9");
    assert_eq!(json["cancellations"][2]["job_id"], "job-z");
    assert_eq!(
        serde_json::from_value::<WorkerProtocolMessage>(json).expect("parses"),
        message
    );
    assert_eq!(
        directive.cancellations()[0].cause(),
        AttemptCancellationCause::OwnershipLost
    );
}

#[test]
fn cancellation_unknown_fields_are_ignored() {
    let message: WorkerProtocolMessage = serde_json::from_str(
        r#"{
          "type":"cancel-attempts",
          "protocol_version":1,
          "worker_id":"worker-a",
          "future_envelope":"ignored",
          "cancellations":[{
            "worker_id":"worker-a",
            "job_id":"job-1",
            "attempt_id":"attempt-1",
            "cause":"ownership_lost",
            "reason":"durable assignment was removed",
            "future_entry":"ignored"
          }]
        }"#,
    )
    .expect("unknown fields remain additive");
    assert!(matches!(message, WorkerProtocolMessage::CancelAttempts(_)));
}

#[test]
fn legacy_missing_attempt_is_exact_none_never_a_wildcard() {
    let message: WorkerProtocolMessage = serde_json::from_str(
        r#"{
          "type":"cancel-attempts",
          "protocol_version":1,
          "worker_id":"worker-a",
          "cancellations":[{
            "worker_id":"worker-a",
            "job_id":"job-1",
            "cause":"ownership_lost",
            "reason":"legacy ownership metadata was removed"
          }]
        }"#,
    )
    .expect("legacy missing attempt remains readable");
    let WorkerProtocolMessage::CancelAttempts(directive) = message else {
        panic!("expected cancellation directive");
    };
    let entry = &directive.cancellations()[0];
    assert_eq!(entry.attempt_id(), None);
    assert!(entry.matches_exact("worker-a", "job-1", None));
    assert!(!entry.matches_exact("worker-a", "job-1", Some("attempt-1")));
}

#[test]
fn invalid_cancellation_envelopes_and_duplicate_identities_are_rejected() {
    for json in [
        r#"{"type":"cancel-attempts","protocol_version":1,"worker_id":"worker-a","cancellations":[]}"#,
        r#"{"type":"cancel-attempts","protocol_version":1,"worker_id":"worker-a","cancellations":[{"worker_id":"worker-b","job_id":"job-1","attempt_id":"attempt-1","cause":"ownership_lost","reason":"lost"}]}"#,
        r#"{"type":"cancel-attempts","protocol_version":1,"worker_id":"worker-a","cancellations":[{"worker_id":"worker-a","job_id":"job-1","attempt_id":"attempt-1","cause":"ownership_lost","reason":"first"},{"worker_id":"worker-a","job_id":"job-1","attempt_id":"attempt-1","cause":"ownership_lost","reason":"second"}]}"#,
        r#"{"type":"cancel-attempts","protocol_version":1,"worker_id":"worker-a","cancellations":[{"worker_id":"worker-a","job_id":"job-1","attempt_id":"  ","cause":"ownership_lost","reason":"lost"}]}"#,
    ] {
        assert!(
            serde_json::from_str::<WorkerProtocolMessage>(json).is_err(),
            "invalid directive parsed: {json}"
        );
    }

    assert!(AttemptCancellation::ownership_lost("worker-a", "job-1", "", "lost").is_err());
    assert!(
        AttemptCancellation::ownership_lost(
            "worker-a",
            "job-1",
            "attempt-1",
            "x".repeat(MAX_ATTEMPT_CANCELLATION_REASON_BYTES + 1),
        )
        .is_err()
    );
}

#[test]
fn cancellation_fixture_and_schema_describe_the_same_required_shape() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/reference/worker-daemon-wire-protocol");
    let fixture: serde_json::Value = serde_json::from_slice(
        &std::fs::read(root.join("examples/cancel-attempts.json")).expect("fixture"),
    )
    .expect("fixture JSON");
    let schema: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("schema.json")).expect("schema"))
            .expect("schema JSON");
    let definition = &schema["$defs"]["cancelAttempts"];
    assert_eq!(definition["properties"]["type"]["const"], fixture["type"]);
    for field in definition["required"].as_array().expect("required fields") {
        assert!(
            fixture.get(field.as_str().expect("field name")).is_some(),
            "fixture omits required field {field}"
        );
    }
    let item = &definition["properties"]["cancellations"]["items"];
    let first = &fixture["cancellations"][0];
    for field in item["required"].as_array().expect("item required fields") {
        assert!(
            first.get(field.as_str().expect("field name")).is_some(),
            "fixture entry omits required field {field}"
        );
    }
    assert_eq!(
        item["properties"]["reason"]["maxLength"],
        MAX_ATTEMPT_CANCELLATION_REASON_BYTES
    );
}
