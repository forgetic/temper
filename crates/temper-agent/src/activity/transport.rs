use std::io::Write as _;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use temper_protocol_activity::{AgentActivityEventV1, AgentActivityFrameV1};

use super::ActivityProjection;

const ACTIVITY_QUEUE_CAPACITY: usize = 256;
const ACTIVITY_CONNECT_TIMEOUT: Duration = Duration::from_millis(200);
const ACTIVITY_WRITE_TIMEOUT: Duration = Duration::from_millis(200);
const ACTIVITY_TERMINAL_FLUSH_TIMEOUT: Duration = Duration::from_millis(250);
const ACTIVITY_FRAME_OVERHEAD_BYTES: usize = 8 * 1024;

struct WriterMessage {
    bytes: Vec<u8>,
    delivered: Option<mpsc::SyncSender<()>>,
}

pub(super) struct ActivityClient {
    sender: mpsc::SyncSender<WriterMessage>,
    dropped: AtomicU64,
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
            dropped: AtomicU64::new(0),
        }
    }
}

impl ActivityProjection for ActivityClient {
    fn emit(&self, frame: &AgentActivityFrameV1) {
        let Ok(mut bytes) = serde_json::to_vec(frame) else {
            return;
        };
        let maximum = temper_protocol_activity::MAX_INLINE_CONTENT_BYTES
            .saturating_add(ACTIVITY_FRAME_OVERHEAD_BYTES);
        if bytes.len().saturating_add(1) > maximum {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        bytes.push(b'\n');

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
        if self
            .sender
            .try_send(WriterMessage { bytes, delivered })
            .is_err()
        {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if let Some(completion) = completion {
            let _ = completion.recv_timeout(ACTIVITY_TERMINAL_FLUSH_TIMEOUT);
        }
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
