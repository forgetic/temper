use std::fs::OpenOptions;
use std::io::Write as _;
use std::net::{Shutdown, TcpStream};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;

use temper_protocol_activity::{
    ACTIVITY_PROTOCOL_VERSION, AgentActivityEventV1, AgentActivityFrameV1, AgentScopeKindV1,
    AgentScopeV1, AssistantMessageV1, BlobAttachmentV1, BlobMediaTypeV1, CaptureModeV1,
    CapturedContentV1, FailureCodeV1, InlineContentV1, RunFinishedV1, RunStartedV1, RunStatusV1,
    UsageV1,
};
use temper_protocol_agent::{
    AgentSessionState, WorkspaceContext, WorkspaceRepository, WorkspaceWorkItem,
};

use super::*;

fn context() -> WorkspaceContext {
    WorkspaceContext {
        trace_context: None,
        repos: vec![WorkspaceRepository {
            id: "forgejo:acme/svc".to_string(),
            owner: "acme".to_string(),
            name: "svc".to_string(),
            default_branch: "main".to_string(),
            dir: "svc".to_string(),
            access: "writable".to_string(),
            base_branch: "main".to_string(),
            branch_hint: Some("agent/run".to_string()),
        }],
        work_item: WorkspaceWorkItem {
            role: "engineer".to_string(),
            queue: "code_ready".to_string(),
            kind: "code".to_string(),
            target: "Issue { number: ItemNumber(308) }".to_string(),
            context: serde_json::json!({
                "artifact": {"type": "issue", "number": 308}
            })
            .to_string(),
        },
        artifact_context: None,
        action: "open_pr".to_string(),
        correlation_key: "pr-for-code-308".to_string(),
        checkout: Some("writable".to_string()),
        allowed_verdicts: Vec::new(),
        verdict_contracts: Default::default(),
        source_metadata: Default::default(),
        guidance: Default::default(),
        pull_request_freshness: None,
        agent_session: Some(AgentSessionState::new("session-308")),
    }
}

fn collector(root: &Path) -> TraceCollector {
    TraceCollector::new(WorkerAgentTraceConfig {
        policy: AgentActivityCapturePolicyV1::default(),
        spool_root: Some(root.to_path_buf()),
    })
}

fn usage_frame(tokens: u64) -> AgentActivityFrameV1 {
    AgentActivityFrameV1 {
        version: ACTIVITY_PROTOCOL_VERSION,
        occurred_at: "2026-07-14T11:09:03.000Z".to_string(),
        elapsed_ms: tokens,
        scope: AgentScopeV1 {
            id: "child-main".to_string(),
            kind: AgentScopeKindV1::Main,
            parent_id: None,
        },
        turn: Some(1),
        event: AgentActivityEventV1::Usage(UsageV1 {
            input_tokens: tokens,
            output_tokens: 1,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        }),
    }
}

#[test]
fn parallel_frames_get_one_gap_free_sequence_and_trusted_identity() {
    let temp = tempfile::tempdir().expect("tempdir");
    let collector = collector(temp.path());
    let run = Arc::new(
        collector
            .begin_run("trusted-job-308", &context())
            .expect("begin")
            .expect("enabled"),
    );
    let count = 24;
    let barrier = Arc::new(Barrier::new(count));
    let mut threads = Vec::new();
    for token in 0..count {
        let run = Arc::clone(&run);
        let barrier = Arc::clone(&barrier);
        threads.push(thread::spawn(move || {
            barrier.wait();
            run.accept_frame(usage_frame(token as u64 + 1))
                .expect("accept frame")
        }));
    }
    let mut assigned = threads
        .into_iter()
        .map(|thread| thread.join().expect("frame thread"))
        .collect::<Vec<_>>();
    assigned.sort_unstable();
    assert_eq!(assigned, (2..=count as u64 + 1).collect::<Vec<_>>());
    run.finish_success(None).expect("finish");
    drop(run);

    let recovered = collector.recover().expect("recover");
    assert_eq!(recovered.len(), 1);
    let recovered = &recovered[0];
    assert_eq!(recovered.events.len(), count + 2);
    assert_ne!(recovered.manifest.main_scope.id, "child-main");
    for (index, event) in recovered.events.iter().enumerate() {
        assert_eq!(event.seq, index as u64 + 1);
        assert_eq!(event.scope.id, recovered.manifest.main_scope.id);
        assert_eq!(event.assignment.job_id, "trusted-job-308");
        assert_eq!(event.assignment.repository, "acme/svc");
        assert_eq!(event.assignment.artifact_ref, "acme/svc#308");
        assert_eq!(event.assignment.role, "engineer");
        assert_eq!(event.assignment.action, "open_pr");
        assert_eq!(event.assignment.correlation_key, "pr-for-code-308");
        assert_eq!(event.agent_session_id.as_deref(), Some("session-308"));
    }
    assert!(matches!(
        recovered.events.first().map(|event| &event.event),
        Some(AgentActivityEventV1::RunStarted(_))
    ));
    assert!(matches!(
        recovered.events.last().map(|event| &event.event),
        Some(AgentActivityEventV1::RunFinished(RunFinishedV1 {
            status: RunStatusV1::Succeeded,
            ..
        }))
    ));
}

#[test]
fn child_root_is_mapped_to_one_unique_canonical_scope_with_correct_parentage() {
    let temp = tempfile::tempdir().expect("tempdir");
    let collector = collector(temp.path());
    let first = collector
        .begin_run("first-run", &context())
        .expect("begin first")
        .expect("enabled");
    let second = collector
        .begin_run("second-run", &context())
        .expect("begin second")
        .expect("enabled");
    assert_ne!(
        first.manifest().main_scope.id,
        second.manifest().main_scope.id
    );

    first.accept_frame(usage_frame(1)).expect("bind child root");
    let mut child = usage_frame(2);
    child.scope = AgentScopeV1 {
        id: "child-scope".to_string(),
        kind: AgentScopeKindV1::SubAgent,
        parent_id: Some("child-main".to_string()),
    };
    first.accept_frame(child).expect("accept child scope");
    let mut second_root = usage_frame(3);
    second_root.scope.id = "another-main".to_string();
    assert!(matches!(
        first.accept_frame(second_root),
        Err(TraceError::InvalidSpool(_))
    ));
    first.finish_success(None).expect("finish first");
    second.finish_success(None).expect("finish second");
    drop(first);
    drop(second);

    let recovered = collector.recover().expect("recover");
    let first = recovered
        .iter()
        .find(|run| run.manifest.assignment.job_id == "first-run")
        .expect("first recovered run");
    assert_eq!(first.events[1].scope, first.manifest.main_scope);
    assert_eq!(
        first.events[2].scope.parent_id.as_deref(),
        Some(first.manifest.main_scope.id.as_str())
    );
}

#[test]
fn endpoint_accepts_bounded_frames_and_rejects_host_events_and_forged_identity() {
    let temp = tempfile::tempdir().expect("tempdir");
    let collector = collector(temp.path());
    let run = collector
        .begin_run("job-endpoint", &context())
        .expect("begin")
        .expect("enabled");
    let endpoint = run.bind_endpoint().expect("bind endpoint");

    send(&endpoint, &serde_json::to_vec(&usage_frame(1)).unwrap());
    let host_frame = AgentActivityFrameV1 {
        event: AgentActivityEventV1::RunStarted(RunStartedV1 {
            capture: CaptureModeV1::Metadata,
        }),
        ..usage_frame(2)
    };
    send(&endpoint, &serde_json::to_vec(&host_frame).unwrap());
    let mut forged = serde_json::to_value(usage_frame(3)).unwrap();
    forged["job_id"] = serde_json::json!("forged-child-job");
    send(&endpoint, &serde_json::to_vec(&forged).unwrap());
    send(&endpoint, b"not-json");
    thread::sleep(Duration::from_millis(80));
    endpoint.stop();
    run.finish_success(None).expect("finish");

    let recovered = collector.recover().expect("recover");
    assert_eq!(recovered[0].events.len(), 3);
    assert!(matches!(
        recovered[0].events[1].event,
        AgentActivityEventV1::Usage(_)
    ));
}

#[test]
fn restart_recovers_blobs_cursor_and_truncates_only_final_fragment() {
    let temp = tempfile::tempdir().expect("tempdir");
    let collector = TraceCollector::new(WorkerAgentTraceConfig {
        policy: AgentActivityCapturePolicyV1 {
            capture: CaptureModeV1::Transcript,
            ..Default::default()
        },
        spool_root: Some(temp.path().to_path_buf()),
    });
    let run = collector
        .begin_run("job-restart", &context())
        .expect("begin")
        .expect("enabled");
    let attachment = BlobAttachmentV1::from_bytes(
        BlobMediaTypeV1::TextMarkdownUtf8,
        b"bounded transcript body",
    );
    run.store_blob(&attachment).expect("store blob");
    let mut frame = usage_frame(1);
    frame.event = AgentActivityEventV1::AssistantMessage(AssistantMessageV1 {
        message_id: "message-1".to_string(),
        content: CapturedContentV1::Blob {
            blob: attachment.blob.clone(),
        },
    });
    assert_eq!(run.accept_frame(frame).expect("accept blob frame"), 2);
    run.acknowledge(2).expect("acknowledge");
    assert_eq!(
        run.finish_failure(FailureCodeV1::ChildProcess, "child crashed", true)
            .unwrap(),
        3
    );
    assert!(matches!(
        run.finish_success(None),
        Err(TraceError::AlreadyTerminal)
    ));
    assert!(
        run.spool_dir()
            .join("blobs")
            .read_dir()
            .unwrap()
            .next()
            .is_some()
    );
    assert!(
        std::fs::metadata(run.spool_dir().join("events.jsonl"))
            .unwrap()
            .len()
            > 0,
        "a partial acknowledgement must not reclaim the unaccepted terminal record"
    );

    let events_path = run.spool_dir().join("events.jsonl");
    let complete_len = std::fs::metadata(&events_path).unwrap().len();
    let mut records = OpenOptions::new().append(true).open(&events_path).unwrap();
    records.write_all(b"{\"incomplete\":").unwrap();
    records.sync_all().unwrap();
    drop(records);
    drop(run);
    // Spools created by the collector-only predecessor did not have an
    // advisory lock file. Forwarding upgrades them in place on first recovery.
    std::fs::remove_file(events_path.with_file_name(".spool.lock")).unwrap();

    let first = collector.recover().expect("first recovery");
    let second = collector.recover().expect("second recovery");
    assert_eq!(first, second, "recovery must not duplicate records");
    assert_eq!(first[0].events.len(), 3);
    assert_eq!(first[0].acknowledged_seq, 2);
    assert_eq!(first[0].blobs, vec![attachment]);
    assert_eq!(std::fs::metadata(events_path).unwrap().len(), complete_len);
    let batch = first[0].pending_batch(10).expect("pending terminal batch");
    assert_eq!(batch.first_seq, 3);
    assert_eq!(batch.events.len(), 1);
    assert!(batch.blobs.is_empty());
    batch.validate().expect("recovered batch validates");
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

    collector
        .acknowledge(&run_id, terminal_seq)
        .expect("acknowledge terminal sequence");
    assert_eq!(
        std::fs::metadata(run_dir.join("events.jsonl"))
            .unwrap()
            .len(),
        0
    );
    assert!(run_dir.join("compacted.json").is_file());
    assert!(run_dir.join("blobs").read_dir().unwrap().next().is_none());

    let recovered = collector.recover().expect("recover compact marker");
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].manifest.run_id, run_id);
    assert_eq!(recovered[0].acknowledged_seq, terminal_seq);
    assert!(recovered[0].events.is_empty());
    assert!(recovered[0].blobs.is_empty());
    assert!(recovered[0].pending_batch(10).is_none());
}

#[test]
fn aggregate_spool_reservations_bound_runs_across_the_worker() {
    let temp = tempfile::tempdir().expect("tempdir");
    let policy = AgentActivityCapturePolicyV1 {
        max_inline_bytes: 1,
        max_blob_bytes: 1,
        max_run_bytes: 5_000,
        ..Default::default()
    };
    let collector = TraceCollector::new(WorkerAgentTraceConfig {
        policy,
        spool_root: Some(temp.path().to_path_buf()),
    });
    let mut runs = Vec::new();
    for index in 0..WORKER_SPOOL_RUN_CAPACITY {
        runs.push(
            collector
                .begin_run(&format!("aggregate-{index}"), &context())
                .expect("reserved run begins")
                .expect("capture enabled"),
        );
    }
    assert!(matches!(
        collector.begin_run("aggregate-exhausted", &context()),
        Err(TraceError::AggregateQuotaExceeded { limit })
            if limit == 5_000 * WORKER_SPOOL_RUN_CAPACITY
    ));
    assert_eq!(runs.len() as u64, WORKER_SPOOL_RUN_CAPACITY);
}

#[test]
fn complete_malformed_record_is_not_silently_truncated() {
    let temp = tempfile::tempdir().expect("tempdir");
    let collector = collector(temp.path());
    let run = collector
        .begin_run("job-corrupt", &context())
        .expect("begin")
        .expect("enabled");
    run.finish_success(None).expect("finish");
    let events_path = run.spool_dir().join("events.jsonl");
    let mut records = OpenOptions::new().append(true).open(events_path).unwrap();
    records.write_all(b"not-json\n").unwrap();
    records.sync_all().unwrap();
    drop(records);
    drop(run);

    assert!(collector.recover().is_err());
}

#[test]
fn metadata_policy_rejects_child_transcript_content() {
    let temp = tempfile::tempdir().expect("tempdir");
    let collector = collector(temp.path());
    let run = collector
        .begin_run("job-metadata-policy", &context())
        .expect("begin")
        .expect("enabled");
    let mut frame = usage_frame(1);
    frame.event = AgentActivityEventV1::AssistantMessage(AssistantMessageV1 {
        message_id: "forbidden-message".to_string(),
        content: CapturedContentV1::Inline(InlineContentV1 {
            text: "must not be stored".to_string(),
            truncated: false,
        }),
    });
    assert!(matches!(
        run.accept_frame(frame),
        Err(TraceError::InvalidSpool(_))
    ));
    run.finish_success(None).expect("finish metadata run");
    drop(run);

    let recovered = collector.recover().expect("recover metadata run");
    assert_eq!(recovered[0].events.len(), 2);
    let serialized = serde_json::to_string(&recovered[0].events).unwrap();
    assert!(!serialized.contains("must not be stored"));
}

#[test]
fn disabled_capture_creates_no_spool_and_quota_keeps_terminal_reserve() {
    let temp = tempfile::tempdir().expect("tempdir");
    let off = TraceCollector::new(WorkerAgentTraceConfig {
        policy: AgentActivityCapturePolicyV1 {
            capture: CaptureModeV1::Off,
            ..Default::default()
        },
        spool_root: Some(temp.path().join("off")),
    });
    assert!(off.begin_run("job-off", &context()).unwrap().is_none());
    assert!(!temp.path().join("off").exists());

    let quota_root = temp.path().join("tight-quota");
    let policy = AgentActivityCapturePolicyV1 {
        max_inline_bytes: 1,
        max_blob_bytes: 1,
        max_run_bytes: 5_000,
        ..Default::default()
    };
    let tight = TraceCollector::new(WorkerAgentTraceConfig {
        policy,
        spool_root: Some(quota_root),
    });
    let run = tight
        .begin_run("job-tight", &context())
        .expect("begin")
        .expect("enabled");
    let mut accepted = 0;
    loop {
        match run.accept_frame(usage_frame(accepted + 1)) {
            Ok(_) => accepted += 1,
            Err(TraceError::QuotaExceeded) => break,
            Err(error) => panic!("unexpected quota error: {error}"),
        }
    }
    assert!(accepted > 0);
    run.finish_failure(FailureCodeV1::Internal, "x", false)
        .expect("terminal reserve remains writable");
}

#[test]
#[cfg(unix)]
fn spool_directories_and_files_are_owner_only() {
    use std::os::unix::fs::PermissionsExt as _;

    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("spool");
    let collector = collector(&root);
    let run = collector
        .begin_run("job-permissions", &context())
        .expect("begin")
        .expect("enabled");
    run.finish_success(None).expect("finish");

    assert_eq!(mode(&root), 0o700);
    assert_eq!(mode(run.spool_dir()), 0o700);
    assert_eq!(mode(&run.spool_dir().join("blobs")), 0o700);
    assert_eq!(mode(&run.spool_dir().join("manifest.json")), 0o600);
    assert_eq!(mode(&run.spool_dir().join("events.jsonl")), 0o600);
    assert_eq!(mode(&run.spool_dir().join("acknowledgement.json")), 0o600);

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }
}

fn send(endpoint: &ActivityEndpoint, payload: &[u8]) {
    let mut stream = TcpStream::connect(endpoint.address()).expect("connect endpoint");
    stream.write_all(payload).expect("write frame");
    stream.write_all(b"\n").expect("write delimiter");
    stream.shutdown(Shutdown::Write).expect("shutdown writer");
}
