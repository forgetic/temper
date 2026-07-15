use std::thread;
use std::time::Duration;

use temper_protocol_activity::{
    ACTIVITY_PROTOCOL_VERSION, AgentActivityChildRecordV1, AgentActivityEventV1,
    AgentActivityFrameV1, AgentScopeKindV1, AgentScopeV1, BlobAttachmentV1, BlobMediaTypeV1,
    CaptureModeV1, CapturedContentV1, InlineContentV1, MAX_BLOB_ATTACHMENT_BYTES,
    PromptCaptureDispositionV1, PromptPreparedV1, PromptSnapshotV1,
};

use super::tests::{context, send};
use super::*;

fn prompt_snapshot(user_bytes: usize) -> PromptSnapshotV1 {
    PromptSnapshotV1 {
        system_prompt: Some("exact worker system prompt".to_string()),
        initial_user_message: "u".repeat(user_bytes),
        tools: Vec::new(),
    }
}

fn captured_prompt_frame(
    user_bytes: usize,
    blob: bool,
) -> (AgentActivityFrameV1, Option<BlobAttachmentV1>) {
    let snapshot = prompt_snapshot(user_bytes);
    let canonical = snapshot.to_canonical_json_bytes().unwrap();
    let tools = snapshot.tools_to_canonical_json_bytes().unwrap();
    let attachment =
        blob.then(|| BlobAttachmentV1::from_bytes(BlobMediaTypeV1::ApplicationJson, &canonical));
    let content = attachment.as_ref().map_or_else(
        || {
            CapturedContentV1::Inline(InlineContentV1 {
                text: String::from_utf8(canonical.clone()).unwrap(),
                truncated: false,
            })
        },
        |attachment| CapturedContentV1::Blob {
            blob: attachment.blob.clone(),
        },
    );
    let frame = AgentActivityFrameV1 {
        version: ACTIVITY_PROTOCOL_VERSION,
        occurred_at: "2026-07-14T11:09:03.000Z".to_string(),
        elapsed_ms: 1,
        scope: AgentScopeV1 {
            id: "child-main".to_string(),
            kind: AgentScopeKindV1::Main,
            parent_id: None,
        },
        turn: Some(0),
        event: AgentActivityEventV1::PromptPrepared(PromptPreparedV1 {
            system_prompt_present: true,
            system_prompt_bytes: "exact worker system prompt".len() as u64,
            initial_user_message_bytes: user_bytes as u64,
            tool_manifest_bytes: tools.len() as u64,
            tool_count: 0,
            original_snapshot_bytes: canonical.len() as u64,
            captured_bytes: canonical.len() as u64,
            disposition: PromptCaptureDispositionV1::Captured,
            content: Some(content),
        }),
    };
    (frame, attachment)
}

fn blob_prompt_record(user_bytes: usize) -> AgentActivityChildRecordV1 {
    let (frame, attachment) = captured_prompt_frame(user_bytes, true);
    AgentActivityChildRecordV1 {
        frame,
        blobs: vec![attachment.expect("blob prompt attachment")],
    }
}

fn omitted_prompt_frame(disposition: PromptCaptureDispositionV1) -> AgentActivityFrameV1 {
    let (mut frame, _) = captured_prompt_frame(8, false);
    let AgentActivityEventV1::PromptPrepared(prompt) = &mut frame.event else {
        unreachable!();
    };
    prompt.disposition = disposition;
    prompt.captured_bytes = 0;
    prompt.content = None;
    frame
}

#[test]
fn endpoint_collects_inline_and_large_prompt_records() {
    let temp = tempfile::tempdir().expect("tempdir");
    let collector = TraceCollector::new(WorkerAgentTraceConfig {
        policy: AgentActivityCapturePolicyV1 {
            capture: CaptureModeV1::Transcript,
            ..Default::default()
        },
        spool_root: Some(temp.path().to_path_buf()),
    });
    let run = collector
        .begin_run("job-prompt-endpoint", &context())
        .expect("begin")
        .expect("enabled");
    let endpoint = run.bind_endpoint().expect("bind endpoint");

    let (inline, _) = captured_prompt_frame(1_024, false);
    send(&endpoint, &serde_json::to_vec(&inline).unwrap());
    let mut large = blob_prompt_record(32 * 1024);
    large.frame.scope = AgentScopeV1 {
        id: "prompt-child".to_string(),
        kind: AgentScopeKindV1::SubAgent,
        parent_id: Some("child-main".to_string()),
    };
    large.validate().expect("large prompt child record");
    send(&endpoint, &serde_json::to_vec(&large).unwrap());

    for _ in 0..200 {
        if collector
            .recover()
            .ok()
            .is_some_and(|runs| runs[0].events.len() == 3)
        {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    endpoint.stop();
    run.finish_success(None).expect("finish");
    drop(run);

    let recovered = collector.recover().expect("recover prompt endpoint spool");
    let prompts = recovered[0]
        .events
        .iter()
        .filter_map(|event| match &event.event {
            AgentActivityEventV1::PromptPrepared(prompt) => Some(prompt),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(prompts.len(), 2);
    assert!(matches!(
        prompts[0].content,
        Some(CapturedContentV1::Inline(_))
    ));
    assert!(matches!(
        prompts[1].content,
        Some(CapturedContentV1::Blob { .. })
    ));
    assert_eq!(recovered[0].blobs, large.blobs);
}

#[test]
fn prompt_records_are_idempotent_and_restart_forwardable() {
    let temp = tempfile::tempdir().expect("tempdir");
    let collector = TraceCollector::new(WorkerAgentTraceConfig {
        policy: AgentActivityCapturePolicyV1 {
            capture: CaptureModeV1::Transcript,
            ..Default::default()
        },
        spool_root: Some(temp.path().to_path_buf()),
    });
    let run = collector
        .begin_run("job-prompt-restart", &context())
        .expect("begin")
        .expect("enabled");
    let record = blob_prompt_record(32 * 1024);
    assert_eq!(run.accept_record(record.clone()).expect("accept prompt"), 2);
    assert_eq!(
        run.accept_record(record.clone()).expect("duplicate prompt"),
        2,
        "an exact retransmission must return its original sequence"
    );

    let conflicting = blob_prompt_record(33 * 1024);
    assert!(matches!(
        run.accept_record(conflicting),
        Err(TraceError::InvalidSpool(_))
    ));
    assert_eq!(run.spool_dir().join("blobs").read_dir().unwrap().count(), 1);
    run.finish_success(None).expect("finish");
    drop(run);

    let recovered = collector.recover().expect("recover prompt spool");
    assert_eq!(recovered[0].events.len(), 3);
    assert_eq!(recovered[0].blobs, record.blobs);
    let batch = recovered[0]
        .pending_batch(10)
        .expect("forward prompt batch");
    assert_eq!(batch.blobs, record.blobs);
    batch.validate().expect("recovered prompt batch validates");
}

#[test]
fn prompt_quota_fallback_preserves_boundary_and_terminal_reserve() {
    let temp = tempfile::tempdir().expect("tempdir");
    let collector = TraceCollector::new(WorkerAgentTraceConfig {
        policy: AgentActivityCapturePolicyV1 {
            capture: CaptureModeV1::Transcript,
            max_run_bytes: 32 * 1024,
            max_blob_bytes: 32 * 1024,
            ..Default::default()
        },
        spool_root: Some(temp.path().to_path_buf()),
    });
    let run = collector
        .begin_run("job-prompt-quota", &context())
        .expect("begin")
        .expect("enabled");
    let record = blob_prompt_record(31 * 1024);
    let original_bytes = record.blobs[0].blob.bytes;
    assert_eq!(run.accept_record(record).expect("accept omitted prompt"), 2);
    assert!(
        run.spool_dir()
            .join("blobs")
            .read_dir()
            .unwrap()
            .next()
            .is_none()
    );
    assert_eq!(run.finish_success(None).expect("terminal reserve"), 3);
    drop(run);

    let recovered = collector.recover().expect("recover quota fallback");
    let AgentActivityEventV1::PromptPrepared(prompt) = &recovered[0].events[1].event else {
        panic!("prompt boundary");
    };
    assert_eq!(prompt.disposition, PromptCaptureDispositionV1::OmittedQuota);
    assert_eq!(prompt.original_snapshot_bytes, original_bytes);
    assert_eq!(prompt.captured_bytes, 0);
    assert!(prompt.content.is_none());
    assert!(recovered[0].blobs.is_empty());
    assert!(matches!(
        recovered[0].events[2].event,
        AgentActivityEventV1::RunFinished(_)
    ));
}

#[test]
fn invalid_prompt_attachments_leave_no_events_or_blobs() {
    let temp = tempfile::tempdir().expect("tempdir");
    let collector = TraceCollector::new(WorkerAgentTraceConfig {
        policy: AgentActivityCapturePolicyV1 {
            capture: CaptureModeV1::Transcript,
            ..Default::default()
        },
        spool_root: Some(temp.path().to_path_buf()),
    });
    let run = collector
        .begin_run("job-invalid-prompt-blobs", &context())
        .expect("begin")
        .expect("enabled");
    let valid = blob_prompt_record(20 * 1024);

    let mut corrupt_digest = valid.clone();
    corrupt_digest.blobs[0].blob.digest.replace_range(7..8, "g");
    let mut corrupt_base64 = valid.clone();
    corrupt_base64.blobs[0].data_base64.push('=');
    let mut missing = valid.clone();
    missing.blobs.clear();
    let mut duplicate = valid.clone();
    duplicate.blobs.push(valid.blobs[0].clone());
    let mut oversized = valid.clone();
    oversized.blobs[0].blob.bytes = MAX_BLOB_ATTACHMENT_BYTES as u64 + 1;
    let mut conflicting = valid.clone();
    conflicting.blobs[0].blob.media_type = BlobMediaTypeV1::TextPlainUtf8;
    let mut unreferenced = valid.clone();
    unreferenced.blobs.push(BlobAttachmentV1::from_bytes(
        BlobMediaTypeV1::ApplicationJson,
        br#"{"unreferenced":true}"#,
    ));

    for invalid in [
        corrupt_digest,
        corrupt_base64,
        missing,
        duplicate,
        oversized,
        conflicting,
        unreferenced,
    ] {
        assert!(run.accept_record(invalid).is_err());
    }
    assert!(
        run.spool_dir()
            .join("blobs")
            .read_dir()
            .unwrap()
            .next()
            .is_none()
    );
    run.finish_success(None)
        .expect("finish after rejected records");
    drop(run);

    let recovered = collector.recover().expect("recover rejected record spool");
    assert_eq!(recovered[0].events.len(), 2);
    assert!(recovered[0].blobs.is_empty());
}

#[test]
fn metadata_accepts_only_policy_omitted_prompts() {
    let temp = tempfile::tempdir().expect("tempdir");
    let collector = TraceCollector::new(WorkerAgentTraceConfig {
        policy: AgentActivityCapturePolicyV1::default(),
        spool_root: Some(temp.path().to_path_buf()),
    });
    let run = collector
        .begin_run("job-prompt-metadata", &context())
        .expect("begin")
        .expect("enabled");

    let (captured, _) = captured_prompt_frame(8, false);
    assert!(matches!(
        run.accept_frame(captured),
        Err(TraceError::InvalidSpool(_))
    ));
    for disposition in [
        PromptCaptureDispositionV1::OmittedLimit,
        PromptCaptureDispositionV1::OmittedQuota,
    ] {
        assert!(matches!(
            run.accept_frame(omitted_prompt_frame(disposition)),
            Err(TraceError::InvalidSpool(_))
        ));
    }
    assert_eq!(
        run.accept_frame(omitted_prompt_frame(
            PromptCaptureDispositionV1::OmittedPolicy,
        ))
        .expect("metadata policy prompt"),
        2
    );
    run.finish_success(None).expect("finish metadata prompt");
    drop(run);

    let recovered = collector.recover().expect("recover metadata prompt");
    let AgentActivityEventV1::PromptPrepared(prompt) = &recovered[0].events[1].event else {
        panic!("metadata prompt boundary");
    };
    assert_eq!(
        prompt.disposition,
        PromptCaptureDispositionV1::OmittedPolicy
    );
    assert!(prompt.content.is_none());
}
