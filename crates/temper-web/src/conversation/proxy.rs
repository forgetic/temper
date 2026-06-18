// SPDX-License-Identifier: MPL-2.0

//! The conversation SSE proxy: poll-to-push translation + POST forwarding.
//!
//! ## What it does
//!
//! The interaction-service exposes conversation history as a *poll* endpoint
//! (`GET /conversations/{id}/events`, returning the full retained batch with
//! monotonic `sequence`s). The chat page wants a *push* stream. The proxy bridges
//! the two: a background poller (see [`super::poller`]) drains each tracked
//! conversation's batch through [`ConversationProxy::poll_events`], which advances
//! a per-conversation cursor and emits each *new* event as an SSE `data:` frame on
//! the shared [`Broadcaster`] — reusing PR D's payload-agnostic fan-out hub
//! verbatim (it just moves pre-serialized strings).
//!
//! ## Resume / dedup (no gap, no dup)
//!
//! The cursor is the highest `sequence` already emitted per conversation. A
//! re-poll of the same batch yields no frames (every `sequence <= cursor`), and a
//! batch that grew yields exactly the tail. On reconnect the client re-attaches to
//! the single `/conversations/events` stream and dedups by `sequence` itself (the
//! chat reducer already does this), so a frame seen twice is harmless — but the
//! cursor keeps the wire quiet in the steady state.
//!
//! ## One stream, many conversations
//!
//! The chat UI subscribes to ONE `/conversations/events` SSE endpoint and
//! demultiplexes by each frame's `conversation_id`. So the proxy owns one
//! `Broadcaster` (mirroring the board's single `/events` hub) and fans every
//! conversation's events out on it; the cursor map is keyed per conversation.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use temper_interaction::ConversationEvent;

use crate::server::sse::{self, Broadcaster};

use super::client::{ClientResponse, InteractionClient};

/// The conversation feed's SSE hub + per-conversation poll cursors, plus the
/// injected interaction client used to forward POSTs and fetch event batches.
pub struct ConversationProxy {
    client: Arc<dyn InteractionClient>,
    broadcaster: Broadcaster,
    /// Highest emitted `sequence` per conversation id (the resume cursor).
    cursors: Mutex<HashMap<String, u64>>,
}

/// One conversation's poll outcome: the SSE frames to broadcast (already framed)
/// and the cursor after applying them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PollOutcome {
    /// Framed SSE `data:` strings for the new events, in `sequence` order.
    pub frames: Vec<String>,
    /// The cursor after this poll (highest emitted `sequence`).
    pub cursor: u64,
}

impl ConversationProxy {
    /// Build a proxy over an injected interaction client, with a fresh hub.
    #[must_use]
    pub fn new(client: Arc<dyn InteractionClient>) -> Self {
        Self {
            client,
            broadcaster: Broadcaster::new(),
            cursors: Mutex::new(HashMap::new()),
        }
    }

    /// The SSE hub for the `/conversations/events` stream (shared with the
    /// connection loop, which subscribes per browser).
    #[must_use]
    pub fn broadcaster(&self) -> Broadcaster {
        self.broadcaster.clone()
    }

    /// The injected client (the binary's blocking HTTP impl, or a test fake).
    #[must_use]
    pub fn client(&self) -> &Arc<dyn InteractionClient> {
        &self.client
    }

    /// Poll one conversation: fetch its batch, advance the cursor, and broadcast
    /// each new event as a `data:` frame. Returns the outcome (frames + cursor)
    /// for observability/tests. A fetch error is logged and treated as no-op.
    pub fn poll_and_broadcast(&self, conversation_id: &str) {
        let batch = match self.client.fetch_events(conversation_id) {
            Ok(batch) => batch,
            Err(error) => {
                tracing::debug!(
                    target: "temper_web",
                    %conversation_id, %error,
                    "conversation poll failed"
                );
                return;
            }
        };
        let outcome = self.poll_events(conversation_id, &batch);
        for frame in &outcome.frames {
            self.broadcaster.broadcast(frame);
        }
    }

    /// Translate a raw events batch into SSE frames for the events newer than the
    /// stored cursor, advancing the cursor. Pure given the cursor state — the unit
    /// test drives this directly with the fixture batch (no client, no socket).
    ///
    /// Accepts either the interaction-service envelope (`{streaming, events:[…]}`)
    /// or a bare `[…]` array, so the fixture and the live response both parse.
    pub fn poll_events(&self, conversation_id: &str, batch_json: &str) -> PollOutcome {
        let events = parse_batch(batch_json);
        let mut cursors = self.cursors.lock().expect("cursors lock");
        let cursor = cursors.entry(conversation_id.to_string()).or_insert(0);
        let mut frames = Vec::new();
        for event in events {
            if event.sequence <= *cursor {
                continue; // already emitted — dedup / resume keeps the wire quiet
            }
            if let Ok(json) = serde_json::to_string(&event) {
                frames.push(sse::data_frame(&json));
            }
            *cursor = event.sequence;
        }
        PollOutcome {
            frames,
            cursor: *cursor,
        }
    }

    /// The current cursor for a conversation (0 if never polled). For tests.
    #[must_use]
    pub fn cursor(&self, conversation_id: &str) -> u64 {
        *self
            .cursors
            .lock()
            .expect("cursors lock")
            .get(conversation_id)
            .unwrap_or(&0)
    }

    /// Forward `POST /conversations/{id}/turns`.
    pub fn forward_turn(
        &self,
        conversation_id: &str,
        body: &str,
    ) -> Result<ClientResponse, String> {
        self.client.post_turn(conversation_id, body)
    }

    /// Forward `POST /conversations/{id}/proposals/{proposal_id}/accept`.
    pub fn forward_accept(
        &self,
        conversation_id: &str,
        proposal_id: &str,
    ) -> Result<ClientResponse, String> {
        self.client.post_accept(conversation_id, proposal_id)
    }

    /// Forward `POST /conversations` (open a new conversation).
    pub fn forward_new_conversation(&self, body: &str) -> Result<ClientResponse, String> {
        self.client.post_new_conversation(body)
    }

    /// The conversations the poller should drain this tick.
    #[must_use]
    pub fn tracked_conversations(&self) -> Vec<String> {
        self.client.tracked_conversations()
    }
}

/// Parse a raw events batch. The interaction-service wraps the list in
/// `{streaming, events:[…]}`; the contract fixture uses the same shape; a bare
/// array is also accepted for robustness. Unparseable input yields no events.
fn parse_batch(batch_json: &str) -> Vec<ConversationEvent> {
    #[derive(serde::Deserialize)]
    struct Envelope {
        #[serde(default)]
        events: Vec<ConversationEvent>,
    }
    if let Ok(envelope) = serde_json::from_str::<Envelope>(batch_json) {
        return envelope.events;
    }
    serde_json::from_str::<Vec<ConversationEvent>>(batch_json).unwrap_or_default()
}

#[cfg(test)]
#[path = "proxy_tests.rs"]
mod proxy_tests;
