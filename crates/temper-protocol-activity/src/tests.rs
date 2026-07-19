use std::collections::BTreeSet;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::*;

mod export;
mod prompt;

fn fixture(name: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(path).expect("fixture is readable"))
        .expect("fixture is valid JSON")
}

fn round_trip<T>(golden: &Value) -> T
where
    T: DeserializeOwned + Serialize,
{
    let parsed: T = serde_json::from_value(golden.clone()).expect("golden value deserializes");
    assert_eq!(
        serde_json::to_value(&parsed).expect("value serializes"),
        *golden
    );
    parsed
}

fn assignment() -> AgentAssignmentIdentityV1 {
    AgentAssignmentIdentityV1 {
        trace_context: None,
        job_id: "job-304".into(),
        repository: "ai/temper".into(),
        artifact_ref: "ai/temper#304".into(),
        role: "engineer".into(),
        action: "code".into(),
        correlation_key: "activity-contract".into(),
    }
}

fn main_scope() -> AgentScopeV1 {
    AgentScopeV1 {
        id: "main".into(),
        kind: AgentScopeKindV1::Main,
        parent_id: None,
    }
}

fn usage_event(seq: u64) -> AgentRunEventV1 {
    AgentRunEventV1 {
        version: ACTIVITY_PROTOCOL_VERSION,
        run_id: "run-304".into(),
        seq,
        occurred_at: "2026-07-13T14:28:03.421Z".into(),
        elapsed_ms: seq * 10,
        assignment: assignment(),
        agent_session_id: Some("session-304".into()),
        scope: main_scope(),
        turn: Some(2),
        event: AgentActivityEventV1::Usage(UsageV1 {
            input_tokens: 10,
            output_tokens: 5,
            cache_read_tokens: 3,
            cache_write_tokens: 1,
        }),
    }
}

fn batch(events: Vec<AgentRunEventV1>) -> AgentActivityBatch {
    AgentActivityBatch {
        version: ACTIVITY_PROTOCOL_VERSION,
        run_id: "run-304".into(),
        first_seq: events[0].seq,
        events,
        blobs: Vec::new(),
    }
}

fn assert_code(result: Result<(), ActivityValidationError>, code: ActivityValidationCode) {
    assert_eq!(result.expect_err("validation should fail").code, code);
}

#[test]
fn every_event_family_has_a_canonical_golden_round_trip() {
    let golden = fixture("event-families.json");
    let events: Vec<AgentActivityEventV1> = round_trip(&golden);
    let expected_types = [
        "run.started",
        "run.finished",
        "scope.started",
        "scope.finished",
        "prompt.prepared",
        "turn.started",
        "turn.finished",
        "model.call.started",
        "model.call.retrying",
        "model.call.finished",
        "assistant.message",
        "output.text.delta",
        "output.thinking.delta",
        "tool.started",
        "tool.finished",
        "steering.applied",
        "usage",
        "trace.gap",
        "run.failed",
    ];
    assert_eq!(
        events
            .iter()
            .map(AgentActivityEventV1::event_type)
            .collect::<Vec<_>>(),
        expected_types
    );

    for (index, event) in events.into_iter().enumerate() {
        let mut canonical = usage_event(index as u64 + 1);
        if matches!(event, AgentActivityEventV1::PromptPrepared(_)) {
            canonical.turn = Some(0);
        }
        canonical.event = event;
        canonical.validate().expect("golden event validates");
    }
}

#[test]
fn retained_scope_finished_without_a_terminal_reason_round_trips() {
    let event: AgentActivityEventV1 = round_trip(&fixture("scope-finished-legacy.json"));
    let AgentActivityEventV1::ScopeFinished(finished) = event else {
        panic!("legacy fixture must be scope.finished");
    };
    assert_eq!(finished.status, ScopeStatusV1::Succeeded);
    assert_eq!(finished.terminal_reason, None);
}

#[test]
fn terminal_reason_goldens_round_trip_and_validate_status_consistency() {
    let events: Vec<AgentActivityEventV1> = round_trip(&fixture("scope-terminal-reasons.json"));
    let expected = [
        (ScopeStatusV1::Succeeded, AgentTerminalReasonV1::Completed),
        (ScopeStatusV1::Failed, AgentTerminalReasonV1::ModelError),
        (ScopeStatusV1::Cancelled, AgentTerminalReasonV1::Aborted),
        (
            ScopeStatusV1::Failed,
            AgentTerminalReasonV1::BudgetExhausted,
        ),
    ];

    for (index, (event, (expected_status, expected_reason))) in
        events.into_iter().zip(expected).enumerate()
    {
        let AgentActivityEventV1::ScopeFinished(finished) = &event else {
            panic!("terminal reason fixture must contain only scope.finished events");
        };
        assert_eq!(finished.status, expected_status);
        assert_eq!(finished.terminal_reason, Some(expected_reason));

        let mut canonical = usage_event(index as u64 + 1);
        canonical.event = event.clone();
        canonical.validate().expect("consistent reason validates");

        let AgentActivityEventV1::ScopeFinished(mut mismatched) = event else {
            unreachable!();
        };
        mismatched.status = if expected_status == ScopeStatusV1::Succeeded {
            ScopeStatusV1::Failed
        } else {
            ScopeStatusV1::Succeeded
        };
        canonical.event = AgentActivityEventV1::ScopeFinished(mismatched);
        assert_code(canonical.validate(), ActivityValidationCode::InvalidEvent);
    }
}

#[test]
fn retry_failures_accept_only_the_fixed_allowlisted_summary() {
    let events: Vec<AgentActivityEventV1> = round_trip(&fixture("event-families.json"));
    let mut canonical = usage_event(1);
    canonical.event = events[8].clone();
    let AgentActivityEventV1::ModelCallRetrying(retry) = &mut canonical.event else {
        panic!("retry fixture");
    };
    let expected_code = retry.failure.code;
    let expected_retryable = retry.failure.retryable;
    retry.failure.message =
        "Authorization: Bearer CREDENTIAL-PROTOCOL-RETRY-SENTINEL-355".to_string();

    assert_code(canonical.validate(), ActivityValidationCode::InvalidEvent);
    canonical.event.sanitize_retry_failure_message();
    canonical
        .validate()
        .expect("sanitized retry event validates");
    let AgentActivityEventV1::ModelCallRetrying(retry) = &canonical.event else {
        unreachable!();
    };
    assert_eq!(retry.failure.message, MODEL_CALL_RETRY_FAILURE_MESSAGE);
    assert_eq!(retry.failure.code, expected_code);
    assert_eq!(retry.failure.retryable, expected_retryable);
}

#[test]
fn event_classification_separates_boundaries_from_droppable_deltas() {
    let events: Vec<AgentActivityEventV1> = round_trip(&fixture("event-families.json"));
    let delta_types = events
        .iter()
        .filter(|event| event.is_droppable())
        .map(AgentActivityEventV1::event_type)
        .collect::<Vec<_>>();
    assert_eq!(delta_types, ["output.text.delta", "output.thinking.delta"]);

    for event in &events {
        if matches!(
            event,
            AgentActivityEventV1::AssistantMessage(_) | AgentActivityEventV1::SteeringApplied(_)
        ) {
            assert_eq!(event.priority(), EventPriorityV1::Normal);
        } else if !event.is_droppable() {
            assert!(event.is_boundary());
        }
    }
    assert_eq!(events[4].classification(), EventClassificationV1::Prompt);
    assert_eq!(events[7].classification(), EventClassificationV1::ModelCall);
    assert_eq!(events[8].classification(), EventClassificationV1::Retry);
    assert_eq!(events[16].classification(), EventClassificationV1::Usage);
    assert_eq!(events[18].classification(), EventClassificationV1::Error);
    assert!(events[1].is_terminal());
    assert!(events[18].is_terminal());
}

#[test]
fn transport_capture_and_blob_goldens_round_trip_and_validate() {
    let golden = fixture("protocol-transport.json");
    let frame: AgentActivityFrameV1 = round_trip(&golden["frame"]);
    frame.validate().expect("frame validates");

    let activity_batch: AgentActivityBatch = round_trip(&golden["batch"]);
    activity_batch.validate().expect("batch validates");
    assert!(matches!(
        activity_batch.events[0].event,
        AgentActivityEventV1::PromptPrepared(_)
    ));
    assert_eq!(
        activity_batch.blobs[0].blob.media_type,
        BlobMediaTypeV1::ApplicationJson
    );

    let acknowledgement: AgentActivityAcknowledgement = round_trip(&golden["acknowledgement"]);
    acknowledgement
        .validate()
        .expect("acknowledgement validates");

    let policies: Vec<AgentActivityCapturePolicyV1> = round_trip(&golden["capture_policies"]);
    assert_eq!(
        policies
            .iter()
            .map(|policy| policy.capture)
            .collect::<Vec<_>>(),
        [
            CaptureModeV1::Off,
            CaptureModeV1::Metadata,
            CaptureModeV1::Transcript,
            CaptureModeV1::Diagnostic,
        ]
    );
    for policy in policies {
        policy.validate().expect("capture policy validates");
    }

    let reference: BlobReferenceV1 = round_trip(&golden["blob_reference"]);
    reference.validate().expect("blob reference validates");
    let attachment: BlobAttachmentV1 = round_trip(&golden["blob_attachment"]);
    attachment.validate().expect("blob attachment validates");
    assert_eq!(attachment.decode().unwrap(), b"hello transcript");
    assert_eq!(
        BlobReferenceV1::for_bytes(BlobMediaTypeV1::TextPlainUtf8, b"hello transcript"),
        reference
    );
}

#[test]
fn child_frame_schema_rejects_trusted_sensitive_and_extension_fields() {
    let golden = fixture("protocol-transport.json");
    let frame = &golden["frame"];
    let keys = frame
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        keys,
        BTreeSet::from([
            "elapsed_ms",
            "event",
            "occurred_at",
            "scope",
            "turn",
            "version"
        ])
    );

    for forbidden in [
        "run_id",
        "seq",
        "job_id",
        "repository",
        "artifact_ref",
        "role",
        "action",
        "correlation_key",
        "agent_session_id",
        "assignment",
        "credentials",
        "headers",
        "environment",
        "extensions",
        "stdout",
        "stderr",
        "workspace_result",
    ] {
        let mut injected = frame.clone();
        injected
            .as_object_mut()
            .unwrap()
            .insert(forbidden.into(), json!({"untrusted": true}));
        assert!(
            serde_json::from_value::<AgentActivityFrameV1>(injected).is_err(),
            "child frame unexpectedly accepted {forbidden}"
        );
    }

    let rendered_frame = serde_json::to_string(frame).unwrap();
    for prompt_body in [
        "You are exact.",
        "Inspect café.",
        "input_schema",
        "Read a file.",
    ] {
        assert!(
            !rendered_frame.contains(prompt_body),
            "metadata frame leaks prompt body {prompt_body}"
        );
    }

    let mut nested_extension = frame.clone();
    nested_extension["event"]["data"]["extensions"] = json!({"anything": true});
    assert!(serde_json::from_value::<AgentActivityFrameV1>(nested_extension).is_err());

    let rendered = serde_json::to_string(&golden).unwrap();
    for forbidden in [
        "credential",
        "authorization",
        "\"headers\"",
        "\"environment\"",
        "\"extensions\"",
        "\"stdout\"",
        "\"stderr\"",
        "workspace_result",
    ] {
        assert!(!rendered.contains(forbidden), "fixture leaks {forbidden}");
    }
}

#[test]
fn invalid_versions_ids_sequences_and_timestamps_are_rejected() {
    let golden = fixture("protocol-transport.json");
    let mut frame: AgentActivityFrameV1 = round_trip(&golden["frame"]);
    frame.version = 2;
    assert_code(frame.validate(), ActivityValidationCode::UnsupportedVersion);

    let mut event = usage_event(1);
    event.version = 2;
    assert_code(event.validate(), ActivityValidationCode::UnsupportedVersion);
    let mut event = usage_event(1);
    event.run_id.clear();
    assert_code(event.validate(), ActivityValidationCode::EmptyIdentifier);
    let mut event = usage_event(1);
    event.seq = 0;
    assert_code(event.validate(), ActivityValidationCode::SequenceZero);
    let mut event = usage_event(1);
    event.occurred_at = "2026-02-30T00:00:00Z".into();
    assert_code(event.validate(), ActivityValidationCode::InvalidTimestamp);
    let mut event = usage_event(1);
    event.assignment.job_id = " ".into();
    assert_code(event.validate(), ActivityValidationCode::EmptyIdentifier);
    let mut event = usage_event(1);
    event.assignment.trace_context = Some(W3cTraceContext {
        traceparent: "00-00000000000000000000000000000000-00f067aa0ba902b7-01".into(),
        tracestate: None,
    });
    assert_code(
        event.validate(),
        ActivityValidationCode::InvalidTraceContext,
    );

    let mut activity_batch = batch(vec![usage_event(1)]);
    activity_batch.version = 2;
    assert_code(
        activity_batch.validate(),
        ActivityValidationCode::UnsupportedVersion,
    );

    let mut acknowledgement: AgentActivityAcknowledgement = round_trip(&golden["acknowledgement"]);
    acknowledgement.version = 2;
    assert_code(
        acknowledgement.validate(),
        ActivityValidationCode::UnsupportedVersion,
    );
    acknowledgement.version = ACTIVITY_PROTOCOL_VERSION;
    acknowledgement.highest_contiguous_seq = 0;
    assert_code(
        acknowledgement.validate(),
        ActivityValidationCode::SequenceZero,
    );

    let policy = AgentActivityCapturePolicyV1 {
        version: 9,
        ..AgentActivityCapturePolicyV1::default()
    };
    assert_code(
        policy.validate(),
        ActivityValidationCode::UnsupportedVersion,
    );
}

#[test]
fn scope_shape_and_complete_ancestry_are_validated() {
    let main = main_scope();
    let child = AgentScopeV1 {
        id: "child".into(),
        kind: AgentScopeKindV1::SubAgent,
        parent_id: Some("main".into()),
    };
    let grandchild = AgentScopeV1 {
        id: "grandchild".into(),
        kind: AgentScopeKindV1::SubAgent,
        parent_id: Some("child".into()),
    };
    validate_scope_ancestry(&[main.clone(), child.clone(), grandchild])
        .expect("complete ancestry validates");

    let missing_parent = AgentScopeV1 {
        id: "orphan".into(),
        kind: AgentScopeKindV1::SubAgent,
        parent_id: Some("missing".into()),
    };
    assert_code(
        validate_scope_ancestry(&[main.clone(), missing_parent]),
        ActivityValidationCode::MalformedScope,
    );
    let cycle_a = AgentScopeV1 {
        id: "a".into(),
        kind: AgentScopeKindV1::SubAgent,
        parent_id: Some("b".into()),
    };
    let cycle_b = AgentScopeV1 {
        id: "b".into(),
        kind: AgentScopeKindV1::SubAgent,
        parent_id: Some("a".into()),
    };
    assert_code(
        validate_scope_ancestry(&[main.clone(), cycle_a, cycle_b]),
        ActivityValidationCode::MalformedScope,
    );
    let second_main = AgentScopeV1 {
        id: "other-main".into(),
        kind: AgentScopeKindV1::Main,
        parent_id: None,
    };
    assert_code(
        validate_scope_ancestry(&[main.clone(), second_main]),
        ActivityValidationCode::MalformedScope,
    );
    let conflicting_child = AgentScopeV1 {
        parent_id: Some("other".into()),
        ..child.clone()
    };
    assert_code(
        validate_scope_ancestry(&[main, child, conflicting_child]),
        ActivityValidationCode::MalformedScope,
    );
}

#[test]
fn batches_reject_gaps_and_mutated_run_identity() {
    let mut non_contiguous = batch(vec![usage_event(7), usage_event(8)]);
    non_contiguous.events[1].seq = 9;
    assert_code(
        non_contiguous.validate(),
        ActivityValidationCode::NonContiguousBatch,
    );

    let mut run_mismatch = batch(vec![usage_event(7), usage_event(8)]);
    run_mismatch.events[1].run_id = "another-run".into();
    assert_code(
        run_mismatch.validate(),
        ActivityValidationCode::RunIdMismatch,
    );

    let mut assignment_mismatch = batch(vec![usage_event(7), usage_event(8)]);
    assignment_mismatch.events[1].assignment.role = "reviewer".into();
    assert_code(
        assignment_mismatch.validate(),
        ActivityValidationCode::AssignmentMismatch,
    );

    let mut session_mismatch = batch(vec![usage_event(7), usage_event(8)]);
    session_mismatch.events[1].agent_session_id = None;
    assert_code(
        session_mismatch.validate(),
        ActivityValidationCode::SessionMismatch,
    );

    let mut elapsed_backwards = batch(vec![usage_event(7), usage_event(8)]);
    elapsed_backwards.events[1].elapsed_ms = 1;
    assert_code(
        elapsed_backwards.validate(),
        ActivityValidationCode::NonMonotonicElapsed,
    );
}

#[test]
fn complete_stream_starts_at_one_and_validates_scope_ancestry() {
    let mut main = usage_event(1);
    main.elapsed_ms = 1;
    let mut child = usage_event(2);
    child.scope = AgentScopeV1 {
        id: "child".into(),
        kind: AgentScopeKindV1::SubAgent,
        parent_id: Some("main".into()),
    };
    validate_run_stream(&[main.clone(), child.clone()]).expect("stream validates");

    main.seq = 2;
    assert_code(
        validate_run_stream(&[main]),
        ActivityValidationCode::NonContiguousBatch,
    );

    child.seq = 1;
    assert_code(
        validate_run_stream(&[child]),
        ActivityValidationCode::MalformedScope,
    );
}

#[test]
fn inline_blob_and_capture_policy_limits_are_enforced() {
    let mut event = usage_event(1);
    event.event = AgentActivityEventV1::OutputTextDelta(OutputDeltaV1 {
        delta: InlineContentV1 {
            text: "x".repeat(MAX_INLINE_CONTENT_BYTES + 1),
            truncated: false,
        },
    });
    assert_code(
        event.validate(),
        ActivityValidationCode::OversizedInlineValue,
    );

    let golden = fixture("protocol-transport.json");
    let activity_batch: AgentActivityBatch = round_trip(&golden["batch"]);
    let mut missing_attachment = activity_batch.clone();
    missing_attachment.blobs.clear();
    assert_code(
        missing_attachment.validate(),
        ActivityValidationCode::BlobReferenceMismatch,
    );
    let mut corrupt_attachment = activity_batch;
    corrupt_attachment.blobs[0].data_base64 = "aGVsbG8=".into();
    assert_code(
        corrupt_attachment.validate(),
        ActivityValidationCode::BlobReferenceMismatch,
    );

    let oversized = BlobReferenceV1 {
        digest: format!("sha256:{}", "a".repeat(64)),
        bytes: MAX_BLOB_ATTACHMENT_BYTES as u64 + 1,
        media_type: BlobMediaTypeV1::TextPlainUtf8,
    };
    assert_code(
        oversized.validate(),
        ActivityValidationCode::InvalidBlobReference,
    );

    let policy = AgentActivityCapturePolicyV1 {
        capture_thinking: true,
        ..AgentActivityCapturePolicyV1::default()
    };
    assert_code(
        policy.validate(),
        ActivityValidationCode::InvalidCapturePolicy,
    );
}

#[test]
fn child_frames_cannot_emit_host_run_or_terminal_events() {
    let golden = fixture("protocol-transport.json");
    let mut frame: AgentActivityFrameV1 = round_trip(&golden["frame"]);
    for event in [
        AgentActivityEventV1::RunStarted(RunStartedV1 {
            capture: CaptureModeV1::Metadata,
        }),
        AgentActivityEventV1::RunFinished(RunFinishedV1 {
            status: RunStatusV1::Succeeded,
            duration_ms: 10,
            stop_reason: Some(StopReasonV1::EndTurn),
        }),
        AgentActivityEventV1::RunFailed(RunFailedV1 {
            failure: FailureInfoV1 {
                code: FailureCodeV1::ChildProcess,
                message: "child exited".into(),
                retryable: false,
            },
        }),
    ] {
        frame.event = event;
        assert_code(frame.validate(), ActivityValidationCode::HostOnlyEvent);
    }
}
