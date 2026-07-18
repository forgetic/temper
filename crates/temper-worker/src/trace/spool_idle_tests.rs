use std::path::Path;

use temper_protocol_activity::{
    AgentActivityCapturePolicyV1, AgentActivityEventV1, AssistantMessageV1, BlobAttachmentV1,
    BlobMediaTypeV1, CaptureModeV1, CapturedContentV1,
};

use super::tests::{context, usage_frame};
use super::*;
use crate::config::WorkerAgentTraceConfig;

fn collector(root: &Path) -> TraceCollector {
    TraceCollector::new(WorkerAgentTraceConfig {
        policy: AgentActivityCapturePolicyV1::default(),
        spool_root: Some(root.to_path_buf()),
    })
}

#[test]
fn fully_acknowledged_terminal_payload_is_replaced_by_a_restart_readable_marker() {
    let temp = tempfile::tempdir().expect("tempdir");
    let collector = TraceCollector::new(WorkerAgentTraceConfig {
        policy: AgentActivityCapturePolicyV1 {
            capture: CaptureModeV1::Transcript,
            ..Default::default()
        },
        spool_root: Some(temp.path().to_path_buf()),
    });
    let run = collector
        .begin_run("job-compact", &context())
        .expect("begin")
        .expect("enabled");
    let attachment = BlobAttachmentV1::from_bytes(
        BlobMediaTypeV1::TextMarkdownUtf8,
        b"payload reclaimed only after terminal acknowledgement",
    );
    run.store_blob(&attachment).expect("store blob");
    let mut frame = usage_frame(1);
    frame.event = AgentActivityEventV1::AssistantMessage(AssistantMessageV1 {
        message_id: "message-compact".to_string(),
        content: CapturedContentV1::Blob {
            blob: attachment.blob,
        },
    });
    run.accept_frame(frame).expect("accept message");
    let terminal_seq = run.finish_success(None).expect("finish");
    let run_id = run.run_id().to_string();
    let run_dir = run.spool_dir().to_path_buf();
    drop(run);

    reset_spool_operation_counts();
    collector
        .acknowledge(&run_id, terminal_seq)
        .expect("acknowledge terminal sequence");
    let compacted_operations = spool_operation_counts();
    assert_eq!(compacted_operations.truncations, 1);
    assert_eq!(compacted_operations.deletions, 1);
    assert!(compacted_operations.file_syncs > 0);
    assert!(compacted_operations.directory_syncs > 0);
    assert_eq!(compacted_operations.permission_changes, 0);
    assert_eq!(
        std::fs::metadata(run_dir.join("events.jsonl"))
            .unwrap()
            .len(),
        0
    );
    assert!(run_dir.join("compacted.json").is_file());
    assert!(run_dir.join("blobs").read_dir().unwrap().next().is_none());

    reset_spool_operation_counts();
    let recovered = collector.recover().expect("recover compact marker");
    assert_eq!(
        spool_operation_counts(),
        TraceSpoolOperationCounts::default(),
        "already-clean compacted recovery must not mutate or sync the spool"
    );
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].manifest.run_id, run_id);
    assert_eq!(recovered[0].acknowledged_seq, terminal_seq);
    assert!(recovered[0].events.is_empty());
    assert!(recovered[0].blobs.is_empty());
    assert!(recovered[0].pending_batch(10).is_none());
}

#[test]
fn compacted_reclamation_rejects_non_regular_blobs_before_mutating_events() {
    let temp = tempfile::tempdir().expect("tempdir");
    let collector = collector(temp.path());
    let run = collector
        .begin_run("job-corrupt-compacted-blob", &context())
        .expect("begin")
        .expect("enabled");
    let terminal = run.finish_success(None).expect("finish");
    let run_dir = run.spool_dir().to_path_buf();
    let events_path = run_dir.join("events.jsonl");
    let durable_events = std::fs::read(&events_path).expect("read terminal events");
    run.acknowledge(terminal).expect("compact terminal run");
    drop(run);

    std::fs::write(&events_path, &durable_events).expect("restore interrupted event reclaim");
    std::fs::create_dir(run_dir.join("blobs").join("unexpected-directory"))
        .expect("create non-regular blob entry");

    reset_spool_operation_counts();
    assert!(collector.recover().is_err());
    assert_eq!(
        std::fs::read(&events_path).expect("read unmodified events"),
        durable_events
    );
    assert_eq!(
        spool_operation_counts(),
        TraceSpoolOperationCounts::default(),
        "validation must finish before compacted payload mutation"
    );
}
