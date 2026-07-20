use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use temper_engine::{
    AgentTraceJournal, AgentTraceRunStatus, AuthenticatedWorkerBinding, RetentionProtection,
    TraceJournalConfig, TraceJournalError,
};
use temper_protocol_activity::{
    ACTIVITY_PROTOCOL_VERSION, AgentActivityBatch, AgentActivityCapturePolicyV1,
    AgentActivityEventV1, AgentAssignmentIdentityV1, AgentRunEventV1, AgentScopeKindV1,
    AgentScopeV1, AssistantMessageV1, BlobAttachmentV1, BlobMediaTypeV1, CaptureModeV1,
    CapturedContentV1, InlineContentV1, ModelCallFinishedV1, ModelCallStatusV1,
    ModelFailureCategoryV1, ModelFailureV1, REDACTED_MODEL_FAILURE_MESSAGE, RunFinishedV1,
    RunStartedV1, RunStatusV1, StopReasonV1, ToolStartedV1, UNKNOWN_MODEL_FAILURE_IDENTITY,
};
use tempfile::TempDir;

const RUN_ID: &str = "run/with-traversal-safe-storage";

fn ts(value: &str) -> DateTime<Utc> {
    value.parse().expect("valid test timestamp")
}

fn clock(now: DateTime<Utc>) -> temper_engine::WallClock {
    Arc::new(move || now)
}

fn mutable_clock(now: DateTime<Utc>) -> (Arc<Mutex<DateTime<Utc>>>, temper_engine::WallClock) {
    let value = Arc::new(Mutex::new(now));
    let captured = Arc::clone(&value);
    (
        value,
        Arc::new(move || *captured.lock().expect("clock lock")),
    )
}

fn policy(capture: CaptureModeV1) -> AgentActivityCapturePolicyV1 {
    AgentActivityCapturePolicyV1 {
        capture,
        max_run_bytes: 32_000,
        max_inline_bytes: 128,
        max_blob_bytes: 512,
        capture_thinking: capture == CaptureModeV1::Diagnostic,
        ..Default::default()
    }
}

fn binding(policy: &AgentActivityCapturePolicyV1) -> AuthenticatedWorkerBinding {
    AuthenticatedWorkerBinding {
        worker_id: "worker-a".to_string(),
        assignment_id: "assignment-a".to_string(),
        assignment: AgentAssignmentIdentityV1 {
            trace_context: None,
            job_id: "job-a".to_string(),
            repository: "ai/temper".to_string(),
            artifact_ref: "ai/temper#309".to_string(),
            role: "engineer".to_string(),
            action: "open_pr".to_string(),
            correlation_key: "pr-for-code-309".to_string(),
        },
        agent_session_id: Some("session-a".to_string()),
        capture_policy: policy.clone(),
    }
}

fn event(
    seq: u64,
    elapsed_ms: u64,
    binding: &AuthenticatedWorkerBinding,
    event: AgentActivityEventV1,
) -> AgentRunEventV1 {
    AgentRunEventV1 {
        version: ACTIVITY_PROTOCOL_VERSION,
        run_id: RUN_ID.to_string(),
        seq,
        occurred_at: format!("2026-01-01T00:00:{:02}Z", seq - 1),
        elapsed_ms,
        assignment: binding.assignment.clone(),
        agent_session_id: binding.agent_session_id.clone(),
        scope: AgentScopeV1 {
            id: "main".to_string(),
            kind: AgentScopeKindV1::Main,
            parent_id: None,
        },
        turn: None,
        event,
    }
}

fn started(seq: u64, binding: &AuthenticatedWorkerBinding) -> AgentRunEventV1 {
    event(
        seq,
        0,
        binding,
        AgentActivityEventV1::RunStarted(RunStartedV1 {
            capture: binding.capture_policy.capture,
        }),
    )
}

fn message(seq: u64, binding: &AuthenticatedWorkerBinding, text: &str) -> AgentRunEventV1 {
    event(
        seq,
        seq * 10,
        binding,
        AgentActivityEventV1::AssistantMessage(AssistantMessageV1 {
            message_id: format!("message-{seq}"),
            content: CapturedContentV1::Inline(InlineContentV1 {
                text: text.to_string(),
                truncated: false,
            }),
        }),
    )
}

fn finished(seq: u64, binding: &AuthenticatedWorkerBinding) -> AgentRunEventV1 {
    event(
        seq,
        seq * 10,
        binding,
        AgentActivityEventV1::RunFinished(RunFinishedV1 {
            status: RunStatusV1::Succeeded,
            duration_ms: seq * 10,
            stop_reason: Some(StopReasonV1::EndTurn),
        }),
    )
}

fn batch(events: Vec<AgentRunEventV1>) -> AgentActivityBatch {
    AgentActivityBatch {
        version: ACTIVITY_PROTOCOL_VERSION,
        run_id: RUN_ID.to_string(),
        first_seq: events[0].seq,
        events,
        blobs: Vec::new(),
    }
}

fn open_journal(temporary: &TempDir, policy: &AgentActivityCapturePolicyV1) -> AgentTraceJournal {
    AgentTraceJournal::open_with_clock(
        TraceJournalConfig {
            root: temporary.path().join("state/agent-traces/journal"),
            policy: policy.clone(),
        },
        clock(ts("2026-01-01T00:00:30Z")),
    )
    .expect("journal opens")
}

#[test]
fn duplicate_gap_binding_and_conflicting_retransmit_are_isolated() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let policy = policy(CaptureModeV1::Metadata);
    let journal = open_journal(&temporary, &policy);
    let binding = binding(&policy);

    let ack = journal
        .ingest(&binding, &batch(vec![started(1, &binding)]))
        .expect("start is durable");
    assert_eq!(ack.highest_contiguous_seq, 1);

    let gap_ack = journal
        .ingest(&binding, &batch(vec![finished(3, &binding)]))
        .expect("a later gap reports the current cursor");
    assert_eq!(gap_ack.highest_contiguous_seq, 1);
    assert_eq!(journal.events(RUN_ID).expect("events read").len(), 1);

    let second = batch(vec![message(2, &binding, "not retained in metadata")]);
    assert_eq!(
        journal
            .ingest(&binding, &second)
            .expect("metadata omission is accepted")
            .highest_contiguous_seq,
        2
    );
    assert_eq!(
        journal
            .ingest(&binding, &second)
            .expect("duplicate is idempotent")
            .highest_contiguous_seq,
        2
    );

    let conflicting = batch(vec![message(2, &binding, "different omitted payload")]);
    assert!(matches!(
        journal.ingest(&binding, &conflicting),
        Err(TraceJournalError::ConflictingRetransmit { seq: 2 })
    ));
    let audit = journal.audit_records().expect("audit reads");
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].kind, "conflicting_retransmit");
    assert_eq!(journal.events(RUN_ID).expect("events read").len(), 2);

    let mut changed_binding = binding.clone();
    changed_binding.worker_id = "worker-b".to_string();
    assert!(matches!(
        journal.ingest(&changed_binding, &batch(vec![started(1, &changed_binding)])),
        Err(TraceJournalError::BindingMismatch)
    ));

    assert_eq!(
        journal
            .ingest(&binding, &batch(vec![finished(3, &binding)]))
            .expect("terminal event remains ingestible")
            .highest_contiguous_seq,
        3
    );
    let summary = journal.summary(RUN_ID).expect("summary reads").unwrap();
    assert_eq!(summary.status, AgentTraceRunStatus::Succeeded);
    assert_eq!(summary.last_accepted_seq, 3);
    assert_eq!(summary.dropped_events, 1);
}

#[test]
fn valid_blobs_are_content_addressed_and_durable() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let policy = policy(CaptureModeV1::Transcript);
    let journal = open_journal(&temporary, &policy);
    let binding = binding(&policy);
    let attachment = BlobAttachmentV1::from_bytes(
        BlobMediaTypeV1::TextPlainUtf8,
        b"bounded captured tool arguments",
    );
    let tool = event(
        2,
        20,
        &binding,
        AgentActivityEventV1::ToolStarted(ToolStartedV1 {
            call_id: "tool-2".to_string(),
            name: "read".to_string(),
            arguments: Some(CapturedContentV1::Blob {
                blob: attachment.blob.clone(),
            }),
        }),
    );
    let activity = AgentActivityBatch {
        version: ACTIVITY_PROTOCOL_VERSION,
        run_id: RUN_ID.to_string(),
        first_seq: 1,
        events: vec![started(1, &binding), tool, finished(3, &binding)],
        blobs: vec![attachment.clone()],
    };

    journal
        .ingest(&binding, &activity)
        .expect("blob batch ingests");
    let digest = attachment
        .blob
        .digest
        .strip_prefix("sha256:")
        .expect("digest prefix");
    let blob_path = journal.run_directory(RUN_ID).join("blobs").join(digest);
    assert_eq!(
        fs::read(blob_path).expect("blob reads"),
        b"bounded captured tool arguments"
    );
    let summary = journal.summary(RUN_ID).expect("summary reads").unwrap();
    assert_eq!(summary.blob_count, 1);
    assert_eq!(summary.blob_bytes, attachment.blob.bytes);
}

#[test]
fn invalid_blob_does_not_bind_or_append_the_run() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let policy = policy(CaptureModeV1::Transcript);
    let journal = open_journal(&temporary, &policy);
    let binding = binding(&policy);
    let attachment = BlobAttachmentV1::from_bytes(BlobMediaTypeV1::TextPlainUtf8, b"captured");
    let reference = attachment.blob.clone();
    let mut invalid_attachment = attachment;
    invalid_attachment.data_base64 = "aW52YWxpZA==".to_string();
    let tool = event(
        2,
        20,
        &binding,
        AgentActivityEventV1::ToolStarted(ToolStartedV1 {
            call_id: "tool-2".to_string(),
            name: "read".to_string(),
            arguments: Some(CapturedContentV1::Blob { blob: reference }),
        }),
    );
    let invalid = AgentActivityBatch {
        version: ACTIVITY_PROTOCOL_VERSION,
        run_id: RUN_ID.to_string(),
        first_seq: 1,
        events: vec![started(1, &binding), tool],
        blobs: vec![invalid_attachment],
    };

    assert!(journal.ingest(&binding, &invalid).is_err());
    assert!(journal.manifest(RUN_ID).expect("manifest lookup").is_none());
    assert!(journal.events(RUN_ID).expect("events lookup").is_empty());
}

#[test]
fn recovery_truncates_only_a_partial_tail_and_acknowledges_a_lost_reply() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let policy = policy(CaptureModeV1::Transcript);
    let binding = binding(&policy);
    let complete = batch(vec![
        started(1, &binding),
        message(2, &binding, "durable response"),
        finished(3, &binding),
    ]);
    let root = temporary.path().join("journal");
    let journal = AgentTraceJournal::open_with_clock(
        TraceJournalConfig {
            root: root.clone(),
            policy: policy.clone(),
        },
        clock(ts("2026-01-01T00:00:30Z")),
    )
    .expect("journal opens");
    journal
        .ingest(&binding, &complete)
        .expect("append succeeds before simulated lost acknowledgement");
    let run_directory = journal.run_directory(RUN_ID);
    fs::remove_file(run_directory.join("summary.json")).expect("summary removed");
    OpenOptions::new()
        .append(true)
        .open(run_directory.join("events.jsonl"))
        .expect("events open")
        .write_all(br#"{"version":1,"run_id":"partial""#)
        .expect("partial append");
    OpenOptions::new()
        .append(true)
        .open(run_directory.join("source-digests.jsonl"))
        .expect("source digest index opens")
        .write_all(br#"{"seq":4,"digest":"partial""#)
        .expect("partial digest append");
    drop(journal);

    let recovered = AgentTraceJournal::open_with_clock(
        TraceJournalConfig { root, policy },
        clock(ts("2026-01-01T00:00:30Z")),
    )
    .expect("journal recovers");
    assert_eq!(
        recovered
            .ingest(&binding, &complete)
            .expect("lost acknowledgement retransmit is deduplicated")
            .highest_contiguous_seq,
        3
    );
    assert_eq!(recovered.events(RUN_ID).expect("events read").len(), 3);
    let bytes = fs::read(run_directory.join("events.jsonl")).expect("events bytes");
    assert!(bytes.ends_with(b"\n"));
    assert_eq!(bytes.iter().filter(|byte| **byte == b'\n').count(), 3);
    assert!(run_directory.join("summary.json").is_file());
}

#[test]
fn policy_and_quota_omit_optional_content_without_blocking_terminal_events() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let policy = AgentActivityCapturePolicyV1 {
        capture: CaptureModeV1::Transcript,
        retention_days: 14,
        max_run_bytes: 4_096,
        max_inline_bytes: 16,
        max_blob_bytes: 32,
        capture_thinking: false,
        version: ACTIVITY_PROTOCOL_VERSION,
    };
    let journal = open_journal(&temporary, &policy);
    let binding = binding(&policy);
    let oversized_blob = BlobAttachmentV1::from_bytes(
        BlobMediaTypeV1::TextPlainUtf8,
        b"this blob is intentionally larger than policy",
    );
    let tool = event(
        3,
        30,
        &binding,
        AgentActivityEventV1::ToolStarted(ToolStartedV1 {
            call_id: "tool-3".to_string(),
            name: "bash".to_string(),
            arguments: Some(CapturedContentV1::Blob {
                blob: oversized_blob.blob.clone(),
            }),
        }),
    );
    let activity = AgentActivityBatch {
        version: ACTIVITY_PROTOCOL_VERSION,
        run_id: RUN_ID.to_string(),
        first_seq: 1,
        events: vec![
            started(1, &binding),
            message(2, &binding, "this inline transcript exceeds policy"),
            tool,
            finished(4, &binding),
        ],
        blobs: vec![oversized_blob],
    };

    assert_eq!(
        journal
            .ingest(&binding, &activity)
            .expect("terminal batch is accepted with omissions")
            .highest_contiguous_seq,
        4
    );
    let events = journal.events(RUN_ID).expect("events read");
    assert!(matches!(events[1].event, AgentActivityEventV1::TraceGap(_)));
    let AgentActivityEventV1::ToolStarted(tool) = &events[2].event else {
        panic!("tool boundary must be preserved")
    };
    assert!(tool.arguments.is_none());
    let summary = journal.summary(RUN_ID).expect("summary reads").unwrap();
    assert_eq!(summary.status, AgentTraceRunStatus::Succeeded);
    assert_eq!(summary.blob_count, 0);
    assert!(summary.stored_bytes <= policy.max_run_bytes);
}

#[test]
fn redacted_model_diagnostics_survive_quota_content_stripping_as_metadata() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let policy = AgentActivityCapturePolicyV1 {
        capture: CaptureModeV1::Transcript,
        retention_days: 14,
        max_run_bytes: 512,
        max_inline_bytes: 128,
        max_blob_bytes: 512,
        capture_thinking: false,
        version: ACTIVITY_PROTOCOL_VERSION,
    };
    let journal = open_journal(&temporary, &policy);
    let binding = binding(&policy);
    let model_failure = event(
        3,
        30,
        &binding,
        AgentActivityEventV1::ModelCallFinished(ModelCallFinishedV1 {
            call_id: "quota-model-failure-531".to_string(),
            attempt: 1,
            status: ModelCallStatusV1::Failed,
            duration_ms: 30,
            time_to_first_token_ms: None,
            stop_reason: Some(StopReasonV1::Error),
            failure: Some(ModelFailureV1 {
                provider: "openai-codex".to_string(),
                model: "gpt-5.6-sol".to_string(),
                category: ModelFailureCategoryV1::RateLimit,
                retryable: true,
                http_status: Some(429),
                provider_request_id: Some("req_quota_531".to_string()),
                provider_error_code: Some("rate_limit".to_string()),
                message: "Provider rate limit exceeded.".to_string(),
                detail_redacted: false,
            }),
        }),
    );
    let activity = batch(vec![
        started(1, &binding),
        message(2, &binding, &"x".repeat(128)),
        model_failure,
        finished(4, &binding),
    ]);

    journal
        .ingest(&binding, &activity)
        .expect("quota stripping keeps required metadata");
    let events = journal.events(RUN_ID).expect("events read");
    assert!(matches!(events[1].event, AgentActivityEventV1::TraceGap(_)));
    let AgentActivityEventV1::ModelCallFinished(finished) = &events[2].event else {
        panic!("model diagnostic boundary survives quota stripping");
    };
    let failure = finished.failure.as_ref().expect("safe model diagnostic");
    assert_eq!(failure.provider, UNKNOWN_MODEL_FAILURE_IDENTITY);
    assert_eq!(failure.model, UNKNOWN_MODEL_FAILURE_IDENTITY);
    assert_eq!(failure.category, ModelFailureCategoryV1::RedactedUnknown);
    assert!(failure.retryable);
    assert_eq!(failure.http_status, Some(429));
    assert_eq!(failure.provider_request_id, None);
    assert_eq!(failure.provider_error_code, None);
    assert_eq!(failure.message, REDACTED_MODEL_FAILURE_MESSAGE);
    assert!(failure.detail_redacted);
}

#[test]
fn retention_uses_the_injected_clock_and_preserves_incomplete_or_recovered_runs() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let mut policy = policy(CaptureModeV1::Metadata);
    policy.retention_days = 1;
    let (clock_value, injected_clock) = mutable_clock(ts("2026-01-01T00:00:30Z"));
    let root = temporary.path().join("journal");
    let journal = AgentTraceJournal::open_with_clock(
        TraceJournalConfig {
            root: root.clone(),
            policy: policy.clone(),
        },
        injected_clock,
    )
    .expect("journal opens");
    let binding = binding(&policy);
    journal
        .ingest(
            &binding,
            &batch(vec![started(1, &binding), finished(2, &binding)]),
        )
        .expect("terminal run ingests");

    let mut active_binding = binding.clone();
    active_binding.assignment_id = "assignment-active".to_string();
    active_binding.assignment.job_id = "job-active".to_string();
    let active_run = "active-run";
    let mut active_start = started(1, &active_binding);
    active_start.run_id = active_run.to_string();
    let active_batch = AgentActivityBatch {
        version: ACTIVITY_PROTOCOL_VERSION,
        run_id: active_run.to_string(),
        first_seq: 1,
        events: vec![active_start],
        blobs: Vec::new(),
    };
    journal
        .ingest(&active_binding, &active_batch)
        .expect("active run ingests");

    *clock_value.lock().expect("clock lock") = ts("2026-01-04T00:00:30Z");
    let protection = RetentionProtection {
        assignment_ids: BTreeSet::from(["assignment-a".to_string()]),
        ..Default::default()
    };
    let protected = journal
        .cleanup_retention(&protection)
        .expect("protected retention succeeds");
    assert_eq!(protected.removed, 0);
    assert_eq!(protected.preserved_in_flight, 1);
    assert_eq!(protected.preserved_incomplete, 1);

    let cleaned = journal
        .cleanup_retention(&RetentionProtection::default())
        .expect("retention succeeds");
    assert_eq!(cleaned.removed, 1);
    assert!(journal.manifest(RUN_ID).expect("manifest lookup").is_none());
    assert!(
        journal
            .manifest(active_run)
            .expect("active manifest lookup")
            .is_some()
    );
}

#[cfg(unix)]
#[test]
fn unix_layout_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = tempfile::tempdir().expect("tempdir");
    let policy = policy(CaptureModeV1::Transcript);
    let journal = open_journal(&temporary, &policy);
    let binding = binding(&policy);
    journal
        .ingest(
            &binding,
            &batch(vec![started(1, &binding), finished(2, &binding)]),
        )
        .expect("run ingests");

    let root = journal.root();
    assert_eq!(
        fs::metadata(root).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(root.join("runs"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    let run = journal.run_directory(RUN_ID);
    assert_eq!(
        fs::metadata(&run).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(run.join("blobs"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    for file in [
        root.join(".journal.lock"),
        root.join(".source-digest.key"),
        run.join("manifest.json"),
        run.join("events.jsonl"),
        run.join("source-digests.jsonl"),
        run.join("summary.json"),
    ] {
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600,
            "{}",
            file.display()
        );
    }
}
