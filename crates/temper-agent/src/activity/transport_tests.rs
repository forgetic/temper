use std::sync::atomic::AtomicU64;

use temper_protocol_activity::{
    ACTIVITY_PROTOCOL_VERSION, AgentActivityEventV1, AgentActivityFrameV1, AgentScopeKindV1,
    AgentScopeV1, InlineContentV1, OutputDeltaV1, TurnStartedV1,
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
fn saturation_discards_only_delta_and_emits_an_ordered_gap() {
    let (sender, receiver) = mpsc::sync_channel(1);
    let client = client(sender);
    client.enqueue(&frame("first"));
    client.enqueue(&frame("dropped"));

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
    client.emit(&frame("live"));
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
