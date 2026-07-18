use std::fs;
use std::path::Path;

use temper_protocol_activity::{
    ACTIVITY_PROTOCOL_VERSION, AgentActivityCapturePolicyV1, AgentActivityEventV1,
    AssistantMessageV1, BlobAttachmentV1, BlobMediaTypeV1, CaptureModeV1, CapturedContentV1,
};

use super::tests::{context, usage_frame};
use super::*;
use crate::config::WorkerAgentTraceConfig;

fn collector(root: &Path, capture: CaptureModeV1) -> TraceCollector {
    TraceCollector::new(WorkerAgentTraceConfig {
        policy: AgentActivityCapturePolicyV1 {
            capture,
            ..Default::default()
        },
        spool_root: Some(root.to_path_buf()),
    })
}

fn forwarding_index(run_dir: &Path) -> serde_json::Value {
    serde_json::from_slice(
        &fs::read(run_dir.join(FORWARDING_INDEX_FILE)).expect("read forwarding index"),
    )
    .expect("parse forwarding index")
}

#[test]
fn fully_acknowledged_non_terminal_run_is_idle_without_payload_reads() {
    let temp = tempfile::tempdir().expect("tempdir");
    let collector = collector(temp.path(), CaptureModeV1::Metadata);
    let run = collector
        .begin_run("indexed-idle", &context())
        .expect("begin")
        .expect("enabled");
    let sequence = run.accept_frame(usage_frame(1)).expect("append usage");
    let run_dir = run.spool_dir().to_path_buf();
    run.acknowledge(sequence).expect("acknowledge live run");
    drop(run);

    let index = forwarding_index(&run_dir);
    assert_eq!(index["version"], 1);
    assert_eq!(index["highest_contiguous_seq"], sequence);
    assert_eq!(
        index["event_end_offset"],
        fs::metadata(run_dir.join("events.jsonl"))
            .expect("event metadata")
            .len()
    );

    reset_event_payload_bytes_read();
    let forwardable = collector
        .recover_forwardable()
        .expect("recover indexed idle run");
    assert_eq!(forwardable.len(), 1);
    assert_eq!(forwardable[0].acknowledged_seq, sequence);
    assert!(forwardable[0].events.is_empty());
    assert!(forwardable[0].blobs.is_empty());
    assert_eq!(event_payload_bytes_read(), 0);

    let public = collector.recover().expect("public full recovery");
    assert_eq!(public[0].events.len(), sequence as usize);
    assert_eq!(public[0].acknowledged_seq, sequence);
}

#[test]
fn append_after_index_recovers_only_suffix_blob_and_exact_ack_boundary() {
    let temp = tempfile::tempdir().expect("tempdir");
    let collector = collector(temp.path(), CaptureModeV1::Transcript);
    let run = collector
        .begin_run("indexed-suffix", &context())
        .expect("begin")
        .expect("enabled");
    let run_id = run.run_id().to_string();
    let run_dir = run.spool_dir().to_path_buf();
    run.acknowledge(1).expect("acknowledge run start");
    let indexed_len = fs::metadata(run_dir.join("events.jsonl"))
        .expect("indexed event metadata")
        .len();

    let attachment = BlobAttachmentV1::from_bytes(
        BlobMediaTypeV1::TextMarkdownUtf8,
        b"only the suffix needs this attachment",
    );
    run.store_blob(&attachment).expect("store suffix blob");
    let mut frame = usage_frame(2);
    frame.event = AgentActivityEventV1::AssistantMessage(AssistantMessageV1 {
        message_id: "indexed-suffix-message".to_string(),
        content: CapturedContentV1::Blob {
            blob: attachment.blob.clone(),
        },
    });
    let suffix_sequence = run.accept_frame(frame).expect("append suffix event");
    let grown_len = fs::metadata(run_dir.join("events.jsonl"))
        .expect("grown event metadata")
        .len();

    reset_event_payload_bytes_read();
    let mut forwardable = collector
        .recover_forwardable()
        .expect("recover forwarding suffix");
    assert_eq!(forwardable.len(), 1);
    let recovered = forwardable.pop().expect("one suffix run");
    assert_eq!(recovered.acknowledged_seq, 1);
    assert_eq!(
        recovered
            .events
            .iter()
            .map(|event| event.seq)
            .collect::<Vec<_>>(),
        vec![suffix_sequence]
    );
    assert_eq!(recovered.blobs, vec![attachment.clone()]);
    assert_eq!(event_payload_bytes_read(), grown_len - indexed_len);
    assert_eq!(
        spool_operation_counts().blob_payload_bytes_read,
        attachment.blob.bytes
    );

    let forwarding_batch = recovered
        .pending_batch_bounded(50, 64 * 1024)
        .expect("suffix batch");
    let (batch, boundaries) = forwarding_batch.into_parts();
    batch.validate().expect("suffix batch validates");
    assert_eq!(batch.first_seq, suffix_sequence);
    let boundary = boundaries
        .into_iter()
        .find(|boundary| boundary.sequence == suffix_sequence)
        .expect("suffix acknowledgement boundary");
    assert_eq!(boundary.event_end_offset, grown_len);
    assert!(!boundary.terminal);
    collector
        .acknowledge_forwarded(&run_id, boundary)
        .expect("persist suffix acknowledgement and index");
    drop(run);

    reset_event_payload_bytes_read();
    let idle = collector
        .recover_forwardable()
        .expect("recover newly idle suffix run");
    assert!(idle[0].events.is_empty());
    assert_eq!(idle[0].acknowledged_seq, suffix_sequence);
    assert_eq!(event_payload_bytes_read(), 0);
}

#[test]
fn newer_cursor_with_older_missing_or_malformed_index_falls_back_and_converges() {
    let temp = tempfile::tempdir().expect("tempdir");
    let collector = collector(temp.path(), CaptureModeV1::Metadata);
    let run = collector
        .begin_run("cursor-index-ordering", &context())
        .expect("begin")
        .expect("enabled");
    let run_id = run.run_id().to_string();
    let run_dir = run.spool_dir().to_path_buf();
    run.acknowledge(1).expect("first acknowledgement");
    let older_index = fs::read(run_dir.join(FORWARDING_INDEX_FILE)).expect("older index");
    let latest_sequence = run
        .accept_frame(usage_frame(2))
        .expect("append newer event");
    run.acknowledge(latest_sequence)
        .expect("newer cursor and index");
    drop(run);

    // Models a crash after the authoritative cursor is durable but before the
    // second sidecar rename: recovery must reject the older derived state.
    fs::write(run_dir.join(FORWARDING_INDEX_FILE), &older_index).expect("restore older index");
    assert_full_fallback_then_idle(&collector, latest_sequence);

    fs::remove_file(run_dir.join(FORWARDING_INDEX_FILE)).expect("remove rebuilt index");
    assert_full_fallback_then_idle(&collector, latest_sequence);

    fs::write(run_dir.join(FORWARDING_INDEX_FILE), b"not-json").expect("malform index");
    assert_full_fallback_then_idle(&collector, latest_sequence);

    let rebuilt = forwarding_index(&run_dir);
    assert_eq!(rebuilt["run_id"], run_id);
    assert_eq!(rebuilt["highest_contiguous_seq"], latest_sequence);
}

#[test]
fn index_ahead_of_authoritative_cursor_retransmits_from_the_cursor() {
    let temp = tempfile::tempdir().expect("tempdir");
    let collector = collector(temp.path(), CaptureModeV1::Metadata);
    let run = collector
        .begin_run("index-ahead", &context())
        .expect("begin")
        .expect("enabled");
    let run_id = run.run_id().to_string();
    let run_dir = run.spool_dir().to_path_buf();
    run.acknowledge(1).expect("acknowledge start");
    let second = run.accept_frame(usage_frame(3)).expect("append second");
    run.acknowledge(second).expect("acknowledge second");
    drop(run);

    let older_cursor = serde_json::to_vec_pretty(&serde_json::json!({
        "version": ACTIVITY_PROTOCOL_VERSION,
        "run_id": run_id,
        "highest_contiguous_seq": 1
    }))
    .expect("serialize older cursor");
    fs::write(run_dir.join("acknowledgement.json"), older_cursor).expect("restore older cursor");

    reset_event_payload_bytes_read();
    let recovered = collector
        .recover_forwardable()
        .expect("recover index ahead of cursor");
    assert!(event_payload_bytes_read() > 0);
    assert_eq!(recovered[0].acknowledged_seq, 1);
    let batch = recovered[0]
        .pending_batch_bounded(50, 64 * 1024)
        .expect("retransmission batch");
    assert_eq!(batch.into_parts().0.first_seq, second);
    let rebuilt = forwarding_index(&run_dir);
    assert_eq!(rebuilt["highest_contiguous_seq"], 1);
}

fn assert_full_fallback_then_idle(collector: &TraceCollector, acknowledged: u64) {
    reset_event_payload_bytes_read();
    let recovered = collector
        .recover_forwardable()
        .expect("full fallback recovery");
    assert!(event_payload_bytes_read() > 0);
    assert_eq!(recovered[0].acknowledged_seq, acknowledged);
    assert!(recovered[0].pending_batch_bounded(50, 64 * 1024).is_none());

    reset_event_payload_bytes_read();
    let idle = collector
        .recover_forwardable()
        .expect("indexed recovery after fallback");
    assert!(idle[0].events.is_empty());
    assert_eq!(idle[0].acknowledged_seq, acknowledged);
    assert_eq!(event_payload_bytes_read(), 0);
}

#[test]
fn scale_fixture_has_no_payload_reads_or_filesystem_mutations_when_idle() {
    const COMPACTED_RUNS: usize = 32;
    const ACKNOWLEDGED_HISTORY_RUNS: usize = 3;
    const EVENTS_PER_HISTORY: usize = 256;

    let temp = tempfile::tempdir().expect("tempdir");
    let collector = collector(temp.path(), CaptureModeV1::Metadata);

    for index in 0..COMPACTED_RUNS {
        let run = collector
            .begin_run(&format!("clean-compacted-{index}"), &context())
            .expect("begin compacted run")
            .expect("enabled compacted run");
        run.accept_frame(usage_frame(index as u64 + 1))
            .expect("append compacted event");
        let terminal = run.finish_success(None).expect("finish compacted run");
        run.acknowledge(terminal).expect("compact acknowledged run");
    }

    for index in 0..ACKNOWLEDGED_HISTORY_RUNS {
        let run = collector
            .begin_run(&format!("large-acknowledged-history-{index}"), &context())
            .expect("begin history run")
            .expect("enabled history run");
        let mut acknowledged = 1;
        for event in 0..EVENTS_PER_HISTORY {
            acknowledged = run
                .accept_frame(usage_frame((index * EVENTS_PER_HISTORY + event + 1) as u64))
                .expect("append history event");
        }
        run.acknowledge(acknowledged)
            .expect("acknowledge retained non-terminal history");
        let run_dir = run.spool_dir().to_path_buf();
        drop(run);

        // Model a legacy acknowledged run. The first convergence pass must
        // parse it once and derive the cheap forwarding boundary.
        fs::remove_file(run_dir.join(FORWARDING_INDEX_FILE))
            .expect("remove derived forwarding index");
    }

    reset_spool_operation_counts();
    let converged = collector
        .recover_forwardable()
        .expect("converge scale fixture");
    assert_eq!(converged.len(), COMPACTED_RUNS + ACKNOWLEDGED_HISTORY_RUNS);
    assert!(
        spool_operation_counts().event_payload_bytes_read > 0,
        "legacy histories should be parsed during convergence"
    );

    reset_spool_operation_counts();
    let idle = collector
        .recover_forwardable()
        .expect("perform second idle backstop");
    assert_eq!(idle.len(), COMPACTED_RUNS + ACKNOWLEDGED_HISTORY_RUNS);
    assert!(
        idle.iter()
            .all(|run| run.events.is_empty() && run.blobs.is_empty())
    );
    assert_eq!(
        spool_operation_counts(),
        TraceSpoolOperationCounts::default(),
        "an idle backstop must remain payload-read and filesystem-mutation free"
    );
}

#[test]
#[cfg(unix)]
fn startup_repairs_legacy_permissions_without_recurring_chmod() {
    use std::os::unix::fs::PermissionsExt as _;

    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("spool");
    let collector = collector(&root, CaptureModeV1::Metadata);
    let run = collector
        .begin_run("legacy-permissions", &context())
        .expect("begin legacy run")
        .expect("enabled legacy run");
    let run_dir = run.spool_dir().to_path_buf();
    run.acknowledge(1).expect("index acknowledged run");
    drop(run);

    let directories = [root.clone(), run_dir.clone(), run_dir.join("blobs")];
    let files = [
        root.join(".spool-root.lock"),
        run_dir.join(".spool.lock"),
        run_dir.join("manifest.json"),
        run_dir.join("events.jsonl"),
        run_dir.join("acknowledgement.json"),
        run_dir.join(FORWARDING_INDEX_FILE),
    ];
    for directory in &directories {
        fs::set_permissions(directory, fs::Permissions::from_mode(0o777))
            .expect("make legacy directory permissive");
    }
    for file in &files {
        fs::set_permissions(file, fs::Permissions::from_mode(0o666))
            .expect("make legacy file permissive");
    }

    reset_spool_operation_counts();
    collector
        .recover_forwardable()
        .expect("ordinary recovery reads legacy spool");
    assert_eq!(spool_operation_counts().permission_changes, 0);
    assert_eq!(mode(&root), 0o777);
    assert_eq!(mode(&run_dir.join(".spool.lock")), 0o666);

    reset_spool_operation_counts();
    collector
        .recover_forwardable_at_startup()
        .expect("startup repairs legacy spool");
    assert!(spool_operation_counts().permission_changes > 0);
    assert_eq!(mode(&root), 0o700);
    assert_eq!(mode(&run_dir), 0o700);
    assert_eq!(mode(&run_dir.join("blobs")), 0o700);
    for file in files {
        assert_eq!(mode(&file), 0o600);
    }

    reset_spool_operation_counts();
    collector
        .recover_forwardable()
        .expect("ordinary idle recovery after repair");
    assert_eq!(
        spool_operation_counts(),
        TraceSpoolOperationCounts::default()
    );

    fn mode(path: &Path) -> u32 {
        fs::metadata(path)
            .expect("permission metadata")
            .permissions()
            .mode()
            & 0o777
    }
}
