use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use temper_protocol_activity::{
    AgentActivityCapturePolicyV1, AgentActivityEventV1, AssistantMessageV1, BlobAttachmentV1,
    BlobMediaTypeV1, CaptureModeV1, CapturedContentV1, FailureCodeV1, RunFailedV1,
};

use super::spool::{ReclamationFault, TERMINALIZATION_MARKER_FILE, set_reclamation_fault};
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

fn abandoned_run(collector: &TraceCollector, job_id: &str) -> PathBuf {
    let run = collector
        .begin_run(job_id, &context())
        .expect("begin run")
        .expect("capture enabled");
    let run_dir = run.spool_dir().to_path_buf();
    drop(run);
    run_dir
}

#[test]
fn abandoned_blob_stream_terminalizes_once_and_compacts_only_after_acknowledgement() {
    let temp = tempfile::tempdir().expect("tempdir");
    let collector = collector(temp.path(), CaptureModeV1::Transcript);
    let run = collector
        .begin_run("abandoned-blob", &context())
        .expect("begin")
        .expect("capture enabled");
    let attachment = BlobAttachmentV1::from_bytes(
        BlobMediaTypeV1::TextMarkdownUtf8,
        b"provider/tool/prompt transcript evidence must remain referenced",
    );
    run.store_blob(&attachment).expect("store referenced blob");
    let mut frame = usage_frame(7);
    frame.event = AgentActivityEventV1::AssistantMessage(AssistantMessageV1 {
        message_id: "abandoned-message".to_string(),
        content: CapturedContentV1::Blob {
            blob: attachment.blob.clone(),
        },
    });
    run.accept_frame(frame).expect("append blob reference");
    let run_id = run.run_id().to_string();
    let run_dir = run.spool_dir().to_path_buf();
    let events_path = run_dir.join("events.jsonl");
    drop(run);

    let mut events = OpenOptions::new()
        .append(true)
        .open(&events_path)
        .expect("open events");
    events
        .write_all(b"{\"provider_stderr\":\"PRIVATE-INCOMPLETE-SENTINEL\"")
        .expect("write incomplete final fragment");
    events.sync_all().expect("sync incomplete fragment");
    drop(events);

    let report = collector
        .reclaim_abandoned_runs(16)
        .expect("reclaim abandoned stream");
    assert_eq!(report.examined_runs, 1);
    assert_eq!(report.terminalized_runs, 1);
    assert_eq!(report.quarantined_runs, 0);
    assert_eq!(report.failed_runs, 0);
    assert_eq!(report.remaining_dirty_runs, 0);
    assert!(run_dir.join(TERMINALIZATION_MARKER_FILE).is_file());

    let recovered = collector.recover().expect("recover terminalized stream");
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].events.len(), 3);
    assert_eq!(recovered[0].blobs, vec![attachment]);
    let terminal = recovered[0].events.last().expect("synthetic terminal");
    let AgentActivityEventV1::RunFailed(RunFailedV1 { failure }) = &terminal.event else {
        panic!("abandoned stream must end in run.failed");
    };
    assert_eq!(failure.code, FailureCodeV1::Internal);
    assert_eq!(failure.message, "agent run failed with a permanent error");
    assert!(!failure.retryable);
    let terminal_json = serde_json::to_string(terminal).expect("serialize terminal");
    assert!(!terminal_json.contains("provider"));
    assert!(!terminal_json.contains("tool"));
    assert!(!terminal_json.contains("stderr"));
    assert!(!terminal_json.contains("prompt"));
    assert!(!terminal_json.contains("transcript"));
    assert!(
        !String::from_utf8_lossy(&fs::read(&events_path).expect("read repaired JSONL"))
            .contains("PRIVATE-INCOMPLETE-SENTINEL")
    );

    let repeated = collector
        .reclaim_abandoned_runs(16)
        .expect("repeat reclamation");
    assert_eq!(repeated.terminalized_runs, 0);
    assert_eq!(
        collector.recover().expect("recover repeated stream")[0]
            .events
            .iter()
            .filter(|event| event.event.is_terminal())
            .count(),
        1
    );
    assert!(fs::metadata(&events_path).expect("event payload").len() > 0);
    assert!(run_dir.join("blobs").read_dir().unwrap().next().is_some());

    collector
        .acknowledge(&run_id, terminal.seq)
        .expect("acknowledge recovered terminal");
    assert_eq!(
        fs::metadata(&events_path).expect("compacted events").len(),
        0
    );
    assert!(run_dir.join("blobs").read_dir().unwrap().next().is_none());
    assert!(run_dir.join("compacted.json").is_file());
    assert!(run_dir.join(TERMINALIZATION_MARKER_FILE).is_file());
    let compacted = collector.recover().expect("recover compacted run");
    assert!(compacted[0].events.is_empty());
}

#[test]
fn live_ownership_fence_skips_without_mutation_then_release_terminalizes_once() {
    let temp = tempfile::tempdir().expect("tempdir");
    let owner = collector(temp.path(), CaptureModeV1::Metadata);
    let reclaimer = collector(temp.path(), CaptureModeV1::Metadata);
    let run = owner
        .begin_run("live-owner", &context())
        .expect("begin")
        .expect("capture enabled");
    let events_path = run.spool_dir().join("events.jsonl");
    let before = fs::read(&events_path).expect("read live events");

    let protected = reclaimer
        .reclaim_abandoned_runs(16)
        .expect("skip protected run");
    assert_eq!(protected.protected_runs, 1);
    assert_eq!(protected.terminalized_runs, 0);
    assert_eq!(protected.remaining_dirty_runs, 1);
    assert_eq!(
        fs::read(&events_path).expect("read unchanged live run"),
        before
    );
    assert_eq!(
        owner.recover().expect("recover live sequence")[0]
            .events
            .len(),
        1
    );

    drop(run);
    let reclaimed = reclaimer
        .reclaim_abandoned_runs(16)
        .expect("reclaim released run");
    assert_eq!(reclaimed.terminalized_runs, 1);
    assert_eq!(reclaimed.remaining_dirty_runs, 0);
    reclaimer
        .reclaim_abandoned_runs(16)
        .expect("repeat released reclamation");
    let events = reclaimer.recover().expect("recover one terminal");
    assert_eq!(events[0].events.len(), 2);
    assert_eq!(
        events[0]
            .events
            .iter()
            .filter(|event| event.event.is_terminal())
            .count(),
        1
    );
}

#[test]
fn malformed_siblings_are_quarantined_collision_safely_without_blocking_healthy_run() {
    let temp = tempfile::tempdir().expect("tempdir");
    let collector = collector(temp.path(), CaptureModeV1::Transcript);
    let healthy = abandoned_run(&collector, "healthy-sibling");

    let bad_manifest = abandoned_run(&collector, "bad-manifest");
    let manifest_bytes = b"{malformed-manifest-bytes";
    fs::write(bad_manifest.join("manifest.json"), manifest_bytes).expect("corrupt manifest");

    let bad_events = abandoned_run(&collector, "bad-events");
    let mut events = OpenOptions::new()
        .append(true)
        .open(bad_events.join("events.jsonl"))
        .expect("open events");
    events
        .write_all(b"malformed-event-record\n")
        .expect("corrupt events");
    events.sync_all().expect("sync corrupt event");

    let bad_cursor = abandoned_run(&collector, "bad-cursor");
    let cursor_bytes = b"{\"highest_contiguous_seq\":999}";
    fs::write(bad_cursor.join("acknowledgement.json"), cursor_bytes).expect("corrupt cursor");

    let bad_blobs = abandoned_run(&collector, "bad-blobs");
    fs::create_dir(bad_blobs.join("blobs").join("unexpected-directory"))
        .expect("corrupt blob layout");

    let quarantine = temp.path().join(TRACE_QUARANTINE_DIR);
    fs::create_dir(&quarantine).expect("precreate quarantine");
    let manifest_name = bad_manifest.file_name().unwrap().to_string_lossy();
    let collision = quarantine.join(format!("{manifest_name}.bad"));
    fs::write(&collision, b"pre-existing quarantined evidence").expect("seed collision");

    let report = collector
        .reclaim_abandoned_runs(16)
        .expect("reclaim mixed siblings");
    assert_eq!(report.examined_runs, 5);
    assert_eq!(report.terminalized_runs, 1);
    assert_eq!(report.quarantined_runs, 4);
    assert_eq!(report.failed_runs, 0);
    assert_eq!(report.remaining_dirty_runs, 0);
    assert!(healthy.is_dir());
    for malformed in [&bad_manifest, &bad_events, &bad_cursor, &bad_blobs] {
        assert!(!malformed.exists(), "malformed active spool was moved");
    }
    assert_eq!(
        fs::read(&collision).unwrap(),
        b"pre-existing quarantined evidence"
    );
    let collision_safe = quarantine.join(format!("{manifest_name}.1.bad"));
    assert_eq!(
        fs::read(collision_safe.join("manifest.json")).unwrap(),
        manifest_bytes
    );
    let cursor_name = bad_cursor.file_name().unwrap().to_string_lossy();
    assert_eq!(
        fs::read(
            quarantine
                .join(format!("{cursor_name}.bad"))
                .join("acknowledgement.json")
        )
        .unwrap(),
        cursor_bytes
    );
    let active = collector.recover().expect("recover healthy active sibling");
    assert_eq!(active.len(), 1);
    assert_eq!(
        active[0].manifest.run_id,
        healthy.file_name().unwrap().to_string_lossy()
    );
    assert!(active[0].events.last().unwrap().event.is_terminal());
    let inventory = collector
        .inventory()
        .expect("inventory quarantine accounting");
    assert_eq!(inventory.outcomes.quarantined_evidence, 5);
    assert_eq!(inventory.outcomes.malformed_runs, 0);
    assert_eq!(
        inventory.logical_reserved_bytes,
        inventory
            .entries
            .iter()
            .filter(|entry| entry.outcome != TraceSpoolOutcome::QuarantinedEvidence)
            .map(|entry| entry.logical_reserved_bytes)
            .sum::<u64>()
    );
    assert!(inventory.quarantined_physical_bytes > 0);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            fs::metadata(&quarantine).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
}

#[test]
fn failures_at_durable_terminal_and_quarantine_boundaries_converge() {
    let marker_temp = tempfile::tempdir().expect("marker tempdir");
    let marker_collector = collector(marker_temp.path(), CaptureModeV1::Metadata);
    let marker_run = abandoned_run(&marker_collector, "marker-boundary");
    set_reclamation_fault(Some(ReclamationFault::AfterMarker));
    let failed_marker = marker_collector
        .reclaim_abandoned_runs(16)
        .expect("isolate marker fault");
    assert_eq!(failed_marker.failed_runs, 1);
    assert_eq!(failed_marker.terminalized_runs, 0);
    assert!(marker_run.join(TERMINALIZATION_MARKER_FILE).is_file());
    assert_eq!(marker_collector.recover().unwrap()[0].events.len(), 1);
    fs::write(
        marker_run.join(TERMINALIZATION_MARKER_FILE),
        b"{\"interrupted_marker\":",
    )
    .expect("simulate a partial marker write");
    let marker_retry = marker_collector
        .reclaim_abandoned_runs(16)
        .expect("retry marker boundary");
    assert_eq!(marker_retry.terminalized_runs, 1);
    assert_eq!(marker_collector.recover().unwrap()[0].events.len(), 2);

    let append_temp = tempfile::tempdir().expect("append tempdir");
    let append_collector = collector(append_temp.path(), CaptureModeV1::Metadata);
    abandoned_run(&append_collector, "append-boundary");
    set_reclamation_fault(Some(ReclamationFault::AfterTerminalAppend));
    let failed_append = append_collector
        .reclaim_abandoned_runs(16)
        .expect("isolate append fault");
    assert_eq!(failed_append.failed_runs, 1);
    let after_append = append_collector
        .recover()
        .expect("durable append survived fault");
    assert_eq!(after_append[0].events.len(), 2);
    assert!(after_append[0].events.last().unwrap().event.is_terminal());
    let append_retry = append_collector
        .reclaim_abandoned_runs(16)
        .expect("retry append boundary");
    assert_eq!(append_retry.terminalized_runs, 0);
    assert_eq!(append_retry.failed_runs, 0);
    assert_eq!(
        append_collector.recover().unwrap()[0]
            .events
            .iter()
            .filter(|event| event.event.is_terminal())
            .count(),
        1
    );

    let quarantine_temp = tempfile::tempdir().expect("quarantine tempdir");
    let quarantine_collector = collector(quarantine_temp.path(), CaptureModeV1::Metadata);
    let malformed = abandoned_run(&quarantine_collector, "quarantine-boundary");
    let malformed_bytes = b"malformed manifest survives rename failure";
    fs::write(malformed.join("manifest.json"), malformed_bytes).expect("corrupt manifest");
    set_reclamation_fault(Some(ReclamationFault::BeforeQuarantineRename));
    let failed_quarantine = quarantine_collector
        .reclaim_abandoned_runs(16)
        .expect("isolate quarantine fault");
    assert_eq!(failed_quarantine.failed_runs, 1);
    assert!(malformed.is_dir());
    assert_eq!(
        fs::read(malformed.join("manifest.json")).unwrap(),
        malformed_bytes
    );
    let quarantine_retry = quarantine_collector
        .reclaim_abandoned_runs(16)
        .expect("retry quarantine boundary");
    assert_eq!(quarantine_retry.quarantined_runs, 1);
    assert!(!malformed.exists());
    let name = malformed.file_name().unwrap().to_string_lossy();
    assert_eq!(
        fs::read(
            quarantine_temp
                .path()
                .join(TRACE_QUARANTINE_DIR)
                .join(format!("{name}.bad"))
                .join("manifest.json")
        )
        .unwrap(),
        malformed_bytes
    );
}

#[test]
fn already_terminal_run_is_not_rewritten_or_marked() {
    let temp = tempfile::tempdir().expect("tempdir");
    let collector = collector(temp.path(), CaptureModeV1::Metadata);
    let run = collector
        .begin_run("already-terminal", &context())
        .expect("begin")
        .expect("capture enabled");
    run.finish_success(None).expect("finish");
    let run_dir = run.spool_dir().to_path_buf();
    let events_path = run_dir.join("events.jsonl");
    let before = fs::read(&events_path).expect("read terminal stream");
    drop(run);

    let report = collector
        .reclaim_abandoned_runs(16)
        .expect("scan terminal run");
    assert_eq!(report.examined_runs, 0);
    assert_eq!(report.terminalized_runs, 0);
    assert_eq!(fs::read(events_path).unwrap(), before);
    assert!(!run_dir.join(TERMINALIZATION_MARKER_FILE).exists());
}
