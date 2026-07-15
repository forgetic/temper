// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::sync::Arc;

use chrono::{DateTime, Utc};

use temper_engine::{
    AgentTraceJournal, AuthenticatedWorkerBinding, TraceJournalConfig, TraceJournalError,
};
use temper_protocol_activity::{
    ACTIVITY_PROTOCOL_VERSION, AgentActivityBatch, AgentActivityCapturePolicyV1,
    AgentActivityEventV1, AgentAssignmentIdentityV1, AgentRunEventV1, AgentScopeKindV1,
    AgentScopeV1, BlobAttachmentV1, BlobMediaTypeV1, CaptureModeV1, CapturedContentV1,
    InlineContentV1, PromptCaptureDispositionV1, PromptPreparedV1, PromptSnapshotV1, RunFinishedV1,
    RunStartedV1, RunStatusV1, StopReasonV1,
};
use tempfile::TempDir;

const RUN_ID: &str = "run-prompt-engine";

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
        worker_id: "worker-prompt".to_string(),
        assignment_id: "assignment-prompt".to_string(),
        assignment: AgentAssignmentIdentityV1 {
            trace_context: None,
            job_id: "job-prompt".to_string(),
            repository: "ai/temper".to_string(),
            artifact_ref: "ai/temper#363".to_string(),
            role: "engineer".to_string(),
            action: "open_pr".to_string(),
            correlation_key: "pr-for-code-363".to_string(),
        },
        agent_session_id: Some("session-prompt".to_string()),
        capture_policy: policy.clone(),
    }
}

fn event(
    seq: u64,
    binding: &AuthenticatedWorkerBinding,
    event: AgentActivityEventV1,
) -> AgentRunEventV1 {
    AgentRunEventV1 {
        version: ACTIVITY_PROTOCOL_VERSION,
        run_id: RUN_ID.to_string(),
        seq,
        occurred_at: format!("2026-01-01T00:00:{:02}Z", seq - 1),
        elapsed_ms: seq * 10,
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

fn started(binding: &AuthenticatedWorkerBinding) -> AgentRunEventV1 {
    event(
        1,
        binding,
        AgentActivityEventV1::RunStarted(RunStartedV1 {
            capture: binding.capture_policy.capture,
        }),
    )
}

fn finished(binding: &AuthenticatedWorkerBinding) -> AgentRunEventV1 {
    event(
        3,
        binding,
        AgentActivityEventV1::RunFinished(RunFinishedV1 {
            status: RunStatusV1::Succeeded,
            duration_ms: 30,
            stop_reason: Some(StopReasonV1::EndTurn),
        }),
    )
}

fn captured_prompt(
    seq: u64,
    binding: &AuthenticatedWorkerBinding,
    user: &str,
    blob: bool,
) -> (AgentRunEventV1, Option<BlobAttachmentV1>) {
    let snapshot = PromptSnapshotV1 {
        system_prompt: Some("exact engine system prompt".to_string()),
        initial_user_message: user.to_string(),
        tools: Vec::new(),
    };
    let canonical = snapshot.to_canonical_json_bytes().expect("snapshot JSON");
    let tool_manifest = snapshot
        .tools_to_canonical_json_bytes()
        .expect("tool manifest JSON");
    let attachment =
        blob.then(|| BlobAttachmentV1::from_bytes(BlobMediaTypeV1::ApplicationJson, &canonical));
    let content = attachment.as_ref().map_or_else(
        || {
            CapturedContentV1::Inline(InlineContentV1 {
                text: String::from_utf8(canonical.clone()).expect("canonical snapshot UTF-8"),
                truncated: false,
            })
        },
        |attachment| CapturedContentV1::Blob {
            blob: attachment.blob.clone(),
        },
    );
    let mut event = event(
        seq,
        binding,
        AgentActivityEventV1::PromptPrepared(PromptPreparedV1 {
            system_prompt_present: true,
            system_prompt_bytes: "exact engine system prompt".len() as u64,
            initial_user_message_bytes: user.len() as u64,
            tool_manifest_bytes: tool_manifest.len() as u64,
            tool_count: 0,
            original_snapshot_bytes: canonical.len() as u64,
            captured_bytes: canonical.len() as u64,
            disposition: PromptCaptureDispositionV1::Captured,
            content: Some(content),
        }),
    );
    event.turn = Some(0);
    (event, attachment)
}

fn omitted_prompt(
    seq: u64,
    binding: &AuthenticatedWorkerBinding,
    disposition: PromptCaptureDispositionV1,
) -> AgentRunEventV1 {
    let (mut event, _) = captured_prompt(seq, binding, "metadata", false);
    let AgentActivityEventV1::PromptPrepared(prompt) = &mut event.event else {
        unreachable!();
    };
    prompt.disposition = disposition;
    prompt.captured_bytes = 0;
    prompt.content = None;
    event
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
    let now: DateTime<Utc> = "2026-01-01T00:00:30Z".parse().expect("clock timestamp");
    AgentTraceJournal::open_with_clock(
        TraceJournalConfig {
            root: temporary.path().join("journal"),
            policy: policy.clone(),
        },
        Arc::new(move || now),
    )
    .expect("journal opens")
}

#[test]
fn prompt_policy_strips_forged_metadata_and_preserves_limit_and_quota_boundaries() {
    let metadata_temp = tempfile::tempdir().expect("tempdir");
    let metadata_policy = policy(CaptureModeV1::Metadata);
    let metadata_journal = open_journal(&metadata_temp, &metadata_policy);
    let metadata_binding = binding(&metadata_policy);
    let (forged_prompt, _) = captured_prompt(2, &metadata_binding, "metadata", false);
    metadata_journal
        .ingest(
            &metadata_binding,
            &batch(vec![started(&metadata_binding), forged_prompt]),
        )
        .expect("engine strips a forged metadata prompt body");
    let events = metadata_journal.events(RUN_ID).expect("metadata events");
    let AgentActivityEventV1::PromptPrepared(prompt) = &events[1].event else {
        panic!("prompt boundary must remain canonical");
    };
    assert_eq!(
        prompt.disposition,
        PromptCaptureDispositionV1::OmittedPolicy
    );
    assert_eq!(prompt.captured_bytes, 0);
    assert!(prompt.content.is_none());

    assert!(matches!(
        metadata_journal.ingest(
            &metadata_binding,
            &batch(vec![omitted_prompt(
                3,
                &metadata_binding,
                PromptCaptureDispositionV1::OmittedQuota,
            )]),
        ),
        Err(TraceJournalError::PolicyViolation(_))
    ));
    assert_eq!(metadata_journal.events(RUN_ID).unwrap().len(), 2);

    let limit_temp = tempfile::tempdir().expect("tempdir");
    let limit_policy = AgentActivityCapturePolicyV1 {
        capture: CaptureModeV1::Transcript,
        max_run_bytes: 4_096,
        max_inline_bytes: 64,
        max_blob_bytes: 512,
        ..Default::default()
    };
    let limit_journal = open_journal(&limit_temp, &limit_policy);
    let limit_binding = binding(&limit_policy);
    let (oversized_inline, _) = captured_prompt(2, &limit_binding, "limit", false);
    limit_journal
        .ingest(
            &limit_binding,
            &batch(vec![started(&limit_binding), oversized_inline]),
        )
        .expect("over-policy prompt body is omitted");
    let limit_events = limit_journal.events(RUN_ID).unwrap();
    let AgentActivityEventV1::PromptPrepared(prompt) = &limit_events[1].event else {
        panic!("prompt boundary");
    };
    assert_eq!(prompt.disposition, PromptCaptureDispositionV1::OmittedLimit);
    assert!(prompt.content.is_none());

    let quota_temp = tempfile::tempdir().expect("tempdir");
    let quota_policy = AgentActivityCapturePolicyV1 {
        capture: CaptureModeV1::Transcript,
        max_run_bytes: 512,
        max_inline_bytes: 128,
        max_blob_bytes: 512,
        ..Default::default()
    };
    let quota_journal = open_journal(&quota_temp, &quota_policy);
    let quota_binding = binding(&quota_policy);
    let (captured, _) = captured_prompt(2, &quota_binding, "quota", false);
    quota_journal
        .ingest(
            &quota_binding,
            &batch(vec![started(&quota_binding), captured]),
        )
        .expect("quota omission keeps required prompt boundary");
    let quota_events = quota_journal.events(RUN_ID).unwrap();
    let AgentActivityEventV1::PromptPrepared(prompt) = &quota_events[1].event else {
        panic!("quota must not turn prompt.prepared into trace.gap");
    };
    assert_eq!(prompt.disposition, PromptCaptureDispositionV1::OmittedQuota);
    assert_eq!(prompt.captured_bytes, 0);
    assert!(prompt.content.is_none());
}

#[test]
fn prompt_blobs_are_validated_deduplicated_recovered_and_conflicts_are_audited() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let policy = policy(CaptureModeV1::Transcript);
    let binding = binding(&policy);
    let (prompt, attachment) = captured_prompt(2, &binding, &"p".repeat(300), true);
    let attachment = attachment.expect("blob prompt");
    let activity = AgentActivityBatch {
        version: ACTIVITY_PROTOCOL_VERSION,
        run_id: RUN_ID.to_string(),
        first_seq: 1,
        events: vec![started(&binding), prompt, finished(&binding)],
        blobs: vec![attachment.clone()],
    };
    let journal = open_journal(&temporary, &policy);
    journal
        .ingest(&binding, &activity)
        .expect("prompt blob ingests");
    journal
        .ingest(&binding, &activity)
        .expect("identical delivery is idempotent");
    assert_eq!(journal.events(RUN_ID).unwrap().len(), 3);
    assert_eq!(journal.summary(RUN_ID).unwrap().unwrap().blob_count, 1);
    drop(journal);

    let recovered = open_journal(&temporary, &policy);
    let run = recovered.run(RUN_ID).unwrap().expect("recovered run");
    assert_eq!(run.events.len(), 3);
    assert_eq!(run.attachments, vec![attachment.clone()]);
    recovered
        .ingest(&binding, &activity)
        .expect("restart retransmission is idempotent");
    assert_eq!(recovered.events(RUN_ID).unwrap().len(), 3);

    let (different_prompt, different_attachment) =
        captured_prompt(2, &binding, &"q".repeat(300), true);
    let conflict = AgentActivityBatch {
        version: ACTIVITY_PROTOCOL_VERSION,
        run_id: RUN_ID.to_string(),
        first_seq: 2,
        events: vec![different_prompt],
        blobs: vec![different_attachment.expect("different blob")],
    };
    assert!(matches!(
        recovered.ingest(&binding, &conflict),
        Err(TraceJournalError::ConflictingRetransmit { seq: 2 })
    ));
    assert_eq!(recovered.audit_records().unwrap().len(), 1);

    let digest = attachment.blob.digest.strip_prefix("sha256:").unwrap();
    fs::write(
        recovered.run_directory(RUN_ID).join("blobs").join(digest),
        b"corrupt prompt bytes",
    )
    .expect("corrupt blob fixture");
    assert!(matches!(
        recovered.run(RUN_ID),
        Err(TraceJournalError::CorruptRun(_))
    ));
}

#[test]
fn engine_revalidates_prompt_blob_snapshot_metadata() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let policy = policy(CaptureModeV1::Transcript);
    let journal = open_journal(&temporary, &policy);
    let binding = binding(&policy);
    let (mut prompt, attachment) = captured_prompt(2, &binding, &"v".repeat(300), true);
    let AgentActivityEventV1::PromptPrepared(metadata) = &mut prompt.event else {
        unreachable!();
    };
    metadata.initial_user_message_bytes += 1;
    let invalid = AgentActivityBatch {
        version: ACTIVITY_PROTOCOL_VERSION,
        run_id: RUN_ID.to_string(),
        first_seq: 1,
        events: vec![started(&binding), prompt],
        blobs: vec![attachment.expect("blob attachment")],
    };

    assert!(matches!(
        journal.ingest(&binding, &invalid),
        Err(TraceJournalError::Validation(_))
    ));
    assert!(journal.manifest(RUN_ID).unwrap().is_none());
}
