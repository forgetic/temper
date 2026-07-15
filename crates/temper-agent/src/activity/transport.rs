use std::io::Write as _;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use temper_protocol_activity::{
    AgentActivityChildRecordV1, AgentActivityEventV1, AgentActivityFrameV1, DroppedEventKindV1,
    MAX_CHILD_ACTIVITY_RECORD_BYTES, TraceGapV1,
};

use super::ActivityProjection;

const ACTIVITY_QUEUE_CAPACITY: usize = 256;
const ACTIVITY_CONNECT_TIMEOUT: Duration = Duration::from_millis(200);
const ACTIVITY_WRITE_TIMEOUT: Duration = Duration::from_millis(200);
const ACTIVITY_TERMINAL_FLUSH_TIMEOUT: Duration = Duration::from_millis(250);
const ACTIVITY_FRAME_OVERHEAD_BYTES: usize = 8 * 1024;
const DELTA_COALESCE_WINDOW: Duration = Duration::from_millis(150);
const DELTA_COALESCE_BYTES: usize = 4 * 1024;

struct WriterMessage {
    bytes: Vec<u8>,
    delivered: Option<mpsc::SyncSender<()>>,
}

struct PendingDelta {
    id: u64,
    frame: AgentActivityFrameV1,
    started: Instant,
}

pub(super) struct ActivityClient {
    sender: mpsc::SyncSender<WriterMessage>,
    pending_delta: Arc<Mutex<Option<PendingDelta>>>,
    next_delta_id: AtomicU64,
    dropped_events: Arc<AtomicU64>,
    dropped_bytes: Arc<AtomicU64>,
    dropped_text: Arc<AtomicU64>,
    dropped_thinking: Arc<AtomicU64>,
}

impl ActivityClient {
    pub(super) fn new(address: &str) -> Self {
        let (sender, receiver) = mpsc::sync_channel(ACTIVITY_QUEUE_CAPACITY);
        let address = address.to_string();
        let _ = std::thread::Builder::new()
            .name("temper-agent-activity".to_string())
            .spawn(move || activity_writer(&address, receiver));
        Self {
            sender,
            pending_delta: Arc::new(Mutex::new(None)),
            next_delta_id: AtomicU64::new(1),
            dropped_events: Arc::new(AtomicU64::new(0)),
            dropped_bytes: Arc::new(AtomicU64::new(0)),
            dropped_text: Arc::new(AtomicU64::new(0)),
            dropped_thinking: Arc::new(AtomicU64::new(0)),
        }
    }

    fn emit_delta(&self, frame: &AgentActivityFrameV1) {
        let mut pending = self.pending_delta.lock().expect("activity delta lock");
        if let Some(existing) = pending.as_mut() {
            if can_coalesce(existing, frame) {
                append_delta(&mut existing.frame, frame);
                return;
            }
        }
        if let Some(existing) = pending.take() {
            self.enqueue(&bare_record(existing.frame));
        }
        let id = self.next_delta_id.fetch_add(1, Ordering::Relaxed);
        *pending = Some(PendingDelta {
            id,
            frame: frame.clone(),
            started: Instant::now(),
        });
        drop(pending);

        let sender = self.sender.clone();
        let pending = Arc::clone(&self.pending_delta);
        let dropped_events = Arc::clone(&self.dropped_events);
        let dropped_bytes = Arc::clone(&self.dropped_bytes);
        let dropped_text = Arc::clone(&self.dropped_text);
        let dropped_thinking = Arc::clone(&self.dropped_thinking);
        let _ = std::thread::Builder::new()
            .name("temper-agent-delta-flush".to_string())
            .spawn(move || {
                std::thread::sleep(DELTA_COALESCE_WINDOW);
                let frame = {
                    let mut pending = pending.lock().expect("activity delta lock");
                    match pending.as_ref() {
                        Some(current) if current.id == id => {
                            pending.take().map(|current| current.frame)
                        }
                        _ => None,
                    }
                };
                if let Some(frame) = frame {
                    enqueue_timed_delta(
                        &sender,
                        &dropped_events,
                        &dropped_bytes,
                        &dropped_text,
                        &dropped_thinking,
                        &frame,
                    );
                }
            });
    }

    fn flush_delta(&self) {
        let pending = self
            .pending_delta
            .lock()
            .expect("activity delta lock")
            .take();
        if let Some(pending) = pending {
            self.enqueue(&bare_record(pending.frame));
        }
    }

    fn enqueue(&self, record: &AgentActivityChildRecordV1) {
        let frame = &record.frame;
        if frame.event.is_droppable() {
            self.flush_gap_before(frame);
        }
        let (serialized, maximum) = if record.blobs.is_empty() {
            (serde_json::to_vec(frame), frame_wire_limit())
        } else {
            if record.validate().is_err() {
                return;
            }
            (serde_json::to_vec(record), MAX_CHILD_ACTIVITY_RECORD_BYTES)
        };
        let Ok(mut bytes) = serialized else {
            return;
        };
        if bytes.len().saturating_add(1) > maximum {
            self.record_drop(frame, bytes.len() as u64);
            return;
        }
        bytes.push(b'\n');
        let encoded_bytes = bytes.len() as u64;

        // A scope terminus is the last child frame for that invocation. Give
        // the writer a short chance to put it on the socket before the agent
        // tears down, without ever turning transport health into a run error.
        let (delivered, completion) =
            if matches!(frame.event, AgentActivityEventV1::ScopeFinished(_)) {
                let (sender, receiver) = mpsc::sync_channel(0);
                (Some(sender), Some(receiver))
            } else {
                (None, None)
            };
        let message = WriterMessage { bytes, delivered };
        let sent = if frame.event.is_droppable() {
            self.sender.try_send(message).is_ok()
        } else {
            // Required/normal events apply bounded-queue backpressure instead
            // of being intentionally dropped. A dead writer disconnects and
            // returns immediately, preserving the product outcome.
            self.sender.send(message).is_ok()
        };
        if !sent {
            self.record_drop(frame, encoded_bytes);
            return;
        }
        if let Some(completion) = completion {
            let _ = completion.recv_timeout(ACTIVITY_TERMINAL_FLUSH_TIMEOUT);
        }
    }

    fn flush_gap_before(&self, frame: &AgentActivityFrameV1) {
        if let Some(gap) = take_gap_frame(
            &self.dropped_events,
            &self.dropped_bytes,
            &self.dropped_text,
            &self.dropped_thinking,
            frame,
        ) {
            self.enqueue(&bare_record(gap));
        }
    }

    fn record_drop(&self, frame: &AgentActivityFrameV1, bytes: u64) {
        // Only deltas are intentionally shed and represented by trace.gap.
        // Required prompt records can fail best-effort transport, but are never
        // rewritten as a misleading delta gap.
        if !frame.event.is_droppable() {
            return;
        }
        record_drop_counters(
            &self.dropped_events,
            &self.dropped_bytes,
            &self.dropped_text,
            &self.dropped_thinking,
            frame,
            bytes,
        );
    }
}

impl ActivityProjection for ActivityClient {
    fn emit(&self, record: &AgentActivityChildRecordV1) {
        let frame = &record.frame;
        if frame.event.is_droppable() {
            self.emit_delta(frame);
            return;
        }
        self.flush_delta();
        self.flush_gap_before(frame);
        self.enqueue(record);
    }
}

impl Drop for ActivityClient {
    fn drop(&mut self) {
        self.flush_delta();
    }
}

fn frame_wire_limit() -> usize {
    temper_protocol_activity::MAX_INLINE_CONTENT_BYTES.saturating_add(ACTIVITY_FRAME_OVERHEAD_BYTES)
}

fn bare_record(frame: AgentActivityFrameV1) -> AgentActivityChildRecordV1 {
    AgentActivityChildRecordV1 {
        frame,
        blobs: Vec::new(),
    }
}

fn enqueue_timed_delta(
    sender: &mpsc::SyncSender<WriterMessage>,
    dropped_events: &AtomicU64,
    dropped_bytes: &AtomicU64,
    dropped_text: &AtomicU64,
    dropped_thinking: &AtomicU64,
    frame: &AgentActivityFrameV1,
) {
    if let Some(gap) = take_gap_frame(
        dropped_events,
        dropped_bytes,
        dropped_text,
        dropped_thinking,
        frame,
    ) {
        let Ok(mut bytes) = serde_json::to_vec(&gap) else {
            return;
        };
        bytes.push(b'\n');
        if sender
            .send(WriterMessage {
                bytes,
                delivered: None,
            })
            .is_err()
        {
            return;
        }
    }
    let Ok(mut bytes) = serde_json::to_vec(frame) else {
        return;
    };
    let maximum = frame_wire_limit();
    if bytes.len().saturating_add(1) > maximum {
        record_drop_counters(
            dropped_events,
            dropped_bytes,
            dropped_text,
            dropped_thinking,
            frame,
            bytes.len() as u64,
        );
        return;
    }
    bytes.push(b'\n');
    let encoded_bytes = bytes.len() as u64;
    if sender
        .try_send(WriterMessage {
            bytes,
            delivered: None,
        })
        .is_err()
    {
        record_drop_counters(
            dropped_events,
            dropped_bytes,
            dropped_text,
            dropped_thinking,
            frame,
            encoded_bytes,
        );
    }
}

fn take_gap_frame(
    dropped_events: &AtomicU64,
    dropped_bytes: &AtomicU64,
    dropped_text: &AtomicU64,
    dropped_thinking: &AtomicU64,
    frame: &AgentActivityFrameV1,
) -> Option<AgentActivityFrameV1> {
    let dropped_events = dropped_events.swap(0, Ordering::AcqRel);
    if dropped_events == 0 {
        return None;
    }
    let dropped_bytes = dropped_bytes.swap(0, Ordering::AcqRel);
    let mut kinds = Vec::new();
    if dropped_text.swap(0, Ordering::AcqRel) > 0 {
        kinds.push(DroppedEventKindV1::TextDelta);
    }
    if dropped_thinking.swap(0, Ordering::AcqRel) > 0 {
        kinds.push(DroppedEventKindV1::ThinkingDelta);
    }
    let mut gap = frame.clone();
    gap.event = AgentActivityEventV1::TraceGap(TraceGapV1 {
        dropped_events,
        dropped_bytes,
        kinds,
    });
    Some(gap)
}

fn record_drop_counters(
    dropped_events: &AtomicU64,
    dropped_bytes: &AtomicU64,
    dropped_text: &AtomicU64,
    dropped_thinking: &AtomicU64,
    frame: &AgentActivityFrameV1,
    bytes: u64,
) {
    // The queue policy intentionally sheds only deltas. Oversized required
    // frames are already prevented by the bounded canonical DTO policy and are
    // counted defensively without changing the run outcome.
    dropped_events.fetch_add(1, Ordering::Relaxed);
    dropped_bytes.fetch_add(bytes, Ordering::Relaxed);
    match frame.event {
        AgentActivityEventV1::OutputTextDelta(_) => {
            dropped_text.fetch_add(1, Ordering::Relaxed);
        }
        AgentActivityEventV1::OutputThinkingDelta(_) => {
            dropped_thinking.fetch_add(1, Ordering::Relaxed);
        }
        _ => {}
    }
}

fn can_coalesce(pending: &PendingDelta, incoming: &AgentActivityFrameV1) -> bool {
    if pending.started.elapsed() > DELTA_COALESCE_WINDOW
        || pending.frame.scope != incoming.scope
        || pending.frame.turn != incoming.turn
    {
        return false;
    }
    match (&pending.frame.event, &incoming.event) {
        (
            AgentActivityEventV1::OutputTextDelta(left),
            AgentActivityEventV1::OutputTextDelta(right),
        )
        | (
            AgentActivityEventV1::OutputThinkingDelta(left),
            AgentActivityEventV1::OutputThinkingDelta(right),
        ) => left.delta.text.len().saturating_add(right.delta.text.len()) <= DELTA_COALESCE_BYTES,
        _ => false,
    }
}

fn append_delta(target: &mut AgentActivityFrameV1, incoming: &AgentActivityFrameV1) {
    match (&mut target.event, &incoming.event) {
        (
            AgentActivityEventV1::OutputTextDelta(left),
            AgentActivityEventV1::OutputTextDelta(right),
        )
        | (
            AgentActivityEventV1::OutputThinkingDelta(left),
            AgentActivityEventV1::OutputThinkingDelta(right),
        ) => {
            left.delta.text.push_str(&right.delta.text);
            left.delta.truncated |= right.delta.truncated;
            target.occurred_at.clone_from(&incoming.occurred_at);
            target.elapsed_ms = incoming.elapsed_ms;
        }
        _ => {}
    }
}

fn activity_writer(address: &str, receiver: mpsc::Receiver<WriterMessage>) {
    let address = address.strip_prefix("tcp://").unwrap_or(address);
    let Ok(addresses) = address.to_socket_addrs() else {
        return;
    };
    let mut stream = None;
    for socket in addresses {
        if let Ok(candidate) = TcpStream::connect_timeout(&socket, ACTIVITY_CONNECT_TIMEOUT) {
            stream = Some(candidate);
            break;
        }
    }
    let Some(mut stream) = stream else {
        return;
    };
    let _ = stream.set_write_timeout(Some(ACTIVITY_WRITE_TIMEOUT));
    while let Ok(message) = receiver.recv() {
        if stream.write_all(&message.bytes).is_err() {
            break;
        }
        if let Some(delivered) = message.delivered {
            let _ = delivered.send(());
        }
    }
}

#[cfg(test)]
#[path = "transport_tests.rs"]
mod tests;
