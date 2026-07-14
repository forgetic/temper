// SPDX-License-Identifier: MPL-2.0

//! SSE framing + a small in-process broadcast hub.
//!
//! This is the reusable SSE machinery: a [`Broadcaster`] that fans serialized
//! frames out to any number of subscriber connections, and the wire-frame
//! helpers ([`data_frame`], [`keep_alive`]). PR E's conversation SSE proxy reuses
//! the same hub and framing — it is deliberately payload-agnostic (it moves
//! pre-serialized strings), so the board feed and a future conversation feed
//! share one implementation.
//!
//! ## Frame format (W3C SSE)
//!
//! - a data event is `data: <json>\n\n` (one logical line; we serialize compact
//!   JSON so no embedded newline splits the frame);
//! - a keep-alive is a comment line `: keep-alive\n\n`, which `EventSource`
//!   ignores but which proves the pipe is live so the client's pulse can tell
//!   idle from dead (UX §4.2 / §6.3).

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

/// The keep-alive comment the server emits (~every 15s) so an idle-but-live pipe
/// is distinguishable from a dead one.
#[must_use]
pub fn keep_alive() -> String {
    ": keep-alive\n\n".to_string()
}

/// Frame a serialized JSON payload as an SSE `data:` event. The payload must be
/// newline-free (compact JSON); each frame ends with the blank-line terminator.
#[must_use]
pub fn data_frame(json: &str) -> String {
    format!("data: {json}\n\n")
}

/// Frame a run event with an SSE id. Native `EventSource` returns this id in
/// `Last-Event-ID` on reconnect, making the run-local sequence the resume cursor.
#[must_use]
pub fn id_data_frame(id: u64, json: &str) -> String {
    format!("id: {id}\ndata: {json}\n\n")
}

/// The HTTP response head for an SSE stream (status line + headers + the blank
/// line that ends the head). Bytes after this are the event stream.
#[must_use]
pub fn sse_response_head() -> &'static str {
    "HTTP/1.1 200 OK\r\n\
     Content-Type: text/event-stream\r\n\
     Cache-Control: no-cache\r\n\
     Connection: keep-alive\r\n\
     X-Accel-Buffering: no\r\n\
     \r\n"
}

/// A subscriber's receiving end of the broadcast — a stream of already-framed
/// SSE strings to write to that connection's socket.
pub type Subscription = Receiver<String>;

/// An in-process fan-out hub: feed it framed SSE strings and it delivers a copy
/// to every live subscriber. Dropped subscribers are pruned lazily on send.
///
/// Cloneable and `Send + Sync` so the accept loop, the feed-pump thread, and the
/// per-connection threads can all share one hub.
#[derive(Clone, Default)]
pub struct Broadcaster {
    subscribers: Arc<Mutex<Vec<Sender<String>>>>,
}

impl Broadcaster {
    /// A fresh hub with no subscribers.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new subscriber, returning its [`Subscription`]. The connection
    /// thread blocks on `recv()` and writes each frame to its socket.
    #[must_use]
    pub fn subscribe(&self) -> Subscription {
        let (tx, rx) = channel();
        self.subscribers
            .lock()
            .expect("broadcaster mutex not poisoned")
            .push(tx);
        rx
    }

    /// Fan a pre-framed SSE string out to every live subscriber, pruning any
    /// whose receiver has been dropped (the connection closed).
    pub fn broadcast(&self, frame: &str) {
        let mut subscribers = self
            .subscribers
            .lock()
            .expect("broadcaster mutex not poisoned");
        subscribers.retain(|tx| tx.send(frame.to_string()).is_ok());
    }

    /// The current live subscriber count (after lazy pruning happens on send).
    #[must_use]
    pub fn subscriber_count(&self) -> usize {
        self.subscribers
            .lock()
            .expect("broadcaster mutex not poisoned")
            .len()
    }
}

#[cfg(test)]
#[path = "sse_tests.rs"]
mod sse_tests;
