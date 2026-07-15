use std::sync::atomic::AtomicU64;

use temper_protocol_activity::{
    ACTIVITY_PROTOCOL_VERSION, AgentActivityEventV1, AgentActivityFrameV1, AgentScopeKindV1,
    AgentScopeV1, BlobAttachmentV1, BlobMediaTypeV1, CapturedContentV1, InlineContentV1,
    OutputDeltaV1, PromptCaptureDispositionV1, PromptPreparedV1, PromptSnapshotV1, TurnStartedV1,
};

use super::*;

fn frame(text: &str) -> AgentActivityFrameV1 {
    AgentActivityFrameV1 {
        version: ACTIVITY_PROTOCOL_VERSION,
        occurred_at: "2026-07-14T11:09:03.000Z".to_string(),
        elapsed_ms: 1,
        scope: AgentScopeV1 {
            id: "main".to_string(),
            kind: AgentScopeKindV1::Main,
            parent_id: None,
        },
        turn: Some(1),
        event: AgentActivityEventV1::OutputTextDelta(OutputDeltaV1 {
            delta: InlineContentV1 {
                text: text.to_string(),
                truncated: false,
            },
        }),
    }
}

fn record(frame: AgentActivityFrameV1) -> AgentActivityChildRecordV1 {
    AgentActivityChildRecordV1 {
        frame,
        blobs: Vec::new(),
    }
}

fn blob_record() -> AgentActivityChildRecordV1 {
    let snapshot = PromptSnapshotV1 {
        system_prompt: Some("exact system".to_string()),
        initial_user_message: "u".repeat(32 * 1024),
        tools: Vec::new(),
    };
    let canonical = snapshot.to_canonical_json_bytes().unwrap();
    let tools = snapshot.tools_to_canonical_json_bytes().unwrap();
    let attachment = BlobAttachmentV1::from_bytes(BlobMediaTypeV1::ApplicationJson, &canonical);
    AgentActivityChildRecordV1 {
        frame: AgentActivityFrameV1 {
            version: ACTIVITY_PROTOCOL_VERSION,
            occurred_at: "2026-07-14T11:09:03.000Z".to_string(),
            elapsed_ms: 2,
            scope: AgentScopeV1 {
                id: "main".to_string(),
                kind: AgentScopeKindV1::Main,
                parent_id: None,
            },
            turn: Some(0),
            event: AgentActivityEventV1::PromptPrepared(PromptPreparedV1 {
                system_prompt_present: true,
                system_prompt_bytes: "exact system".len() as u64,
                initial_user_message_bytes: 32 * 1024,
                tool_manifest_bytes: tools.len() as u64,
                tool_count: 0,
                original_snapshot_bytes: canonical.len() as u64,
                captured_bytes: canonical.len() as u64,
                disposition: PromptCaptureDispositionV1::Captured,
                content: Some(CapturedContentV1::Blob {
                    blob: attachment.blob.clone(),
                }),
            }),
        },
        blobs: vec![attachment],
    }
}

fn client(sender: mpsc::SyncSender<WriterMessage>) -> ActivityClient {
    ActivityClient {
        sender,
        pending_delta: Arc::new(Mutex::new(None)),
        next_delta_id: AtomicU64::new(1),
        dropped_events: Arc::new(AtomicU64::new(0)),
        dropped_bytes: Arc::new(AtomicU64::new(0)),
        dropped_text: Arc::new(AtomicU64::new(0)),
        dropped_thinking: Arc::new(AtomicU64::new(0)),
    }
}

#[test]
fn ordinary_records_keep_the_bare_frame_wire_shape() {
    let (sender, receiver) = mpsc::sync_channel(1);
    let client = client(sender);
    let mut boundary = frame("");
    boundary.event = AgentActivityEventV1::TurnStarted(TurnStartedV1 {});
    client.enqueue(&record(boundary.clone()));

    let message = receiver.recv().expect("ordinary frame");
    let value: serde_json::Value = serde_json::from_slice(&message.bytes).unwrap();
    assert!(value.get("frame").is_none());
    assert!(value.get("blobs").is_none());
    assert_eq!(
        serde_json::from_value::<AgentActivityFrameV1>(value).unwrap(),
        boundary
    );
}

#[test]
fn blob_prompt_is_one_separately_bounded_queue_item() {
    let (sender, receiver) = mpsc::sync_channel(1);
    let client = client(sender);
    let record = blob_record();
    record.validate().expect("test prompt record");
    client.enqueue(&record);

    let message = receiver.recv().expect("blob prompt");
    assert!(message.bytes.len() > frame_wire_limit());
    assert!(message.bytes.len() <= MAX_CHILD_ACTIVITY_RECORD_BYTES + 1);
    let decoded: AgentActivityChildRecordV1 =
        serde_json::from_slice(&message.bytes).expect("attachment-bearing record");
    assert_eq!(decoded, record);
    assert!(
        receiver.try_recv().is_err(),
        "record must be one queue item"
    );
}

#[test]
fn required_blob_prompt_backpressures_instead_of_becoming_a_gap() {
    let (sender, receiver) = mpsc::sync_channel(1);
    let client = client(sender);
    client.enqueue(&record(frame("queued delta")));

    let dropped_events = Arc::clone(&client.dropped_events);
    let (finished_tx, finished_rx) = mpsc::sync_channel(0);
    let prompt = blob_record();
    let writer = std::thread::spawn(move || {
        client.enqueue(&prompt);
        let _ = finished_tx.send(());
    });

    std::thread::sleep(Duration::from_millis(20));
    assert!(matches!(
        finished_rx.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    let first = receiver.recv().expect("queued delta");
    let first: AgentActivityFrameV1 = serde_json::from_slice(&first.bytes).unwrap();
    assert!(matches!(
        first.event,
        AgentActivityEventV1::OutputTextDelta(_)
    ));
    finished_rx
        .recv_timeout(Duration::from_millis(200))
        .expect("required prompt unblocks after queue capacity is available");
    let second = receiver.recv().expect("required prompt");
    let second: AgentActivityChildRecordV1 = serde_json::from_slice(&second.bytes).unwrap();
    assert!(matches!(
        second.frame.event,
        AgentActivityEventV1::PromptPrepared(_)
    ));
    assert_eq!(dropped_events.load(Ordering::Relaxed), 0);
    writer.join().unwrap();
}

#[test]
fn saturation_discards_only_delta_and_emits_an_ordered_gap() {
    let (sender, receiver) = mpsc::sync_channel(1);
    let client = client(sender);
    client.enqueue(&record(frame("first")));
    client.enqueue(&record(frame("dropped")));

    let first = receiver.recv().expect("first delta");
    let first: AgentActivityFrameV1 = serde_json::from_slice(&first.bytes).expect("first frame");
    assert!(matches!(
        first.event,
        AgentActivityEventV1::OutputTextDelta(_)
    ));

    let mut boundary = frame("");
    boundary.event = AgentActivityEventV1::TurnStarted(TurnStartedV1 {});
    client.flush_gap_before(&boundary);
    let gap = receiver.recv().expect("gap");
    let gap: AgentActivityFrameV1 = serde_json::from_slice(&gap.bytes).expect("gap frame");
    let AgentActivityEventV1::TraceGap(gap) = gap.event else {
        panic!("expected trace.gap");
    };
    assert_eq!(gap.dropped_events, 1);
    assert!(gap.dropped_bytes > 0);
    assert_eq!(gap.kinds, vec![DroppedEventKindV1::TextDelta]);
}

#[test]
fn delta_flushes_when_the_coalescing_window_expires() {
    let (sender, receiver) = mpsc::sync_channel(1);
    let client = client(sender);
    client.emit(&record(frame("live")));
    let message = receiver
        .recv_timeout(Duration::from_millis(300))
        .expect("timed delta flush");
    let flushed: AgentActivityFrameV1 =
        serde_json::from_slice(&message.bytes).expect("flushed frame");
    assert!(matches!(
        flushed.event,
        AgentActivityEventV1::OutputTextDelta(_)
    ));
}

#[test]
fn deltas_coalesce_within_the_time_and_four_kibibyte_bounds() {
    let mut pending = PendingDelta {
        id: 1,
        frame: frame(&"a".repeat(2_000)),
        started: Instant::now(),
    };
    let incoming = frame(&"b".repeat(2_000));
    assert!(can_coalesce(&pending, &incoming));
    append_delta(&mut pending.frame, &incoming);
    let AgentActivityEventV1::OutputTextDelta(delta) = &pending.frame.event else {
        panic!("text delta");
    };
    assert_eq!(delta.delta.text.len(), 4_000);
    assert!(!can_coalesce(&pending, &frame(&"c".repeat(97))));
}
