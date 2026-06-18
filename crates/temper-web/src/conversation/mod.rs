// SPDX-License-Identifier: MPL-2.0

//! Feed 2 — the conversation SSE proxy (UX §6.1 feed 2 / §7).
//!
//! temper-web proxies the interaction-service's poll-based conversation API as a
//! per-page push surface:
//!
//! - **SSE**: `GET /conversations/events` streams each new [`ConversationEvent`]
//!   as one `data:` frame. A background [`poller`] fetches the upstream
//!   `GET /conversations/{id}/events` batch on an interval, advances a cursor by
//!   `sequence`, and broadcasts new events on PR D's [`Broadcaster`]
//!   (reused verbatim — payload-agnostic, moves pre-serialized strings). The chat
//!   UI subscribes to the single stream and demultiplexes by `conversation_id`.
//! - **POST proxies**: `POST /conversations`, `/conversations/{id}/turns`, and
//!   `/conversations/{id}/proposals/{proposal_id}/accept` forward to the
//!   interaction-service and pass its response through.
//!
//! The interaction client is **injected** ([`client::InteractionClient`]); the
//! blocking HTTP impl lives in the binary, so library code stays env-free and
//! tests run with an in-memory fake. The proxy is provider-neutral: profile ids
//! pass through as opaque JSON, never hardcoded.
//!
//! [`ConversationEvent`]: temper_interaction::ConversationEvent
//! [`Broadcaster`]: crate::server::sse::Broadcaster

pub mod client;
pub mod poller;
pub mod proxy;

pub use client::{ClientResponse, InteractionClient};
pub use poller::{poll_tick, spawn_poller};
pub use proxy::{ConversationProxy, PollOutcome};

#[cfg(test)]
pub mod test_support;
