// SPDX-License-Identifier: MPL-2.0

//! The injected interaction-service client seam.
//!
//! temper-web's conversation proxy talks to the interaction-service over its
//! poll-based REST API, but the library never opens a socket itself: it takes an
//! [`InteractionClient`] injected at the binary entry (the blocking HTTP client
//! lives in `src/bin/temper-web/interaction_client.rs`, like `daemon_client.rs`).
//! This keeps the library env-free and lets tests drive the proxy with an
//! in-memory fake — no live service required.
//!
//! The client is deliberately thin: it moves raw JSON request/response bodies.
//! Typed parsing of the events batch happens in [`super::proxy`] using the
//! `temper-interaction` wire types (one source of truth for the JSON contract).

/// A buffered HTTP-ish response from the interaction-service: the status code and
/// the raw body. The proxy forwards these straight back to the browser, so we
/// keep the upstream status (e.g. 201 for a created conversation) intact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientResponse {
    /// Upstream HTTP status code.
    pub status: u16,
    /// Raw response body (JSON, forwarded verbatim).
    pub body: String,
}

impl ClientResponse {
    /// A 200 response carrying `body`.
    #[must_use]
    pub fn ok(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            body: body.into(),
        }
    }
}

/// A blocking client to the interaction-service. The production implementation is
/// a tiny HTTP client built at the binary entry; tests inject a fake.
///
/// All methods are best-effort and return the upstream status + body so the proxy
/// can pass the response through. Transport failures surface as `Err(String)` and
/// the proxy maps them to a 502.
pub trait InteractionClient: Send + Sync {
    /// `GET /conversations/{id}/events` — the full retained event batch for a
    /// conversation. The proxy advances its cursor over the returned `sequence`s,
    /// so the upstream returning the whole list (it ignores `after`) is fine.
    fn fetch_events(&self, conversation_id: &str) -> Result<String, String>;

    /// `POST /conversations/{id}/turns` with the given raw JSON body.
    fn post_turn(&self, conversation_id: &str, body: &str) -> Result<ClientResponse, String>;

    /// `POST /conversations/{id}/proposals/{proposal_id}/accept`.
    fn post_accept(
        &self,
        conversation_id: &str,
        proposal_id: &str,
    ) -> Result<ClientResponse, String>;

    /// `POST /conversations` with the given raw JSON body (opens a conversation).
    fn post_new_conversation(&self, body: &str) -> Result<ClientResponse, String>;

    /// The conversations the poller should track. Returns the ids whose events
    /// the poll loop fetches each tick. A real client may discover these from the
    /// service; the simplest implementation returns the ids opened via this proxy.
    fn tracked_conversations(&self) -> Vec<String>;
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod client_tests;
