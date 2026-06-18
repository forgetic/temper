// SPDX-License-Identifier: MPL-2.0

//! Test-only fakes for the conversation proxy.
//!
//! `FakeInteractionClient` stands in for the live interaction-service so the
//! proxy, poller, and POST passthrough can be exercised with no socket. It
//! records the POSTs it received (so tests assert the forward) and serves a
//! canned events batch per conversation (so the poller has something to drain).

use std::sync::Mutex;

use super::client::{ClientResponse, InteractionClient};

/// One recorded POST the proxy forwarded, for forward-assertion in tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordedPost {
    /// `POST /conversations/{id}/turns` with the raw body.
    Turn {
        conversation_id: String,
        body: String,
    },
    /// `POST /conversations/{id}/proposals/{proposal_id}/accept`.
    Accept {
        conversation_id: String,
        proposal_id: String,
    },
    /// `POST /conversations` with the raw body.
    NewConversation { body: String },
}

/// An in-memory interaction-service stand-in.
pub struct FakeInteractionClient {
    /// Per-conversation canned events JSON (the `GET .../events` response body).
    events: Mutex<Vec<(String, String)>>,
    /// Forwarded POSTs, in order.
    posts: Mutex<Vec<RecordedPost>>,
    /// Canned response for every POST.
    response: Mutex<ClientResponse>,
}

impl Default for FakeInteractionClient {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeInteractionClient {
    /// A fresh fake with no events and a default 200 `{}` POST response.
    #[must_use]
    pub fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            posts: Mutex::new(Vec::new()),
            response: Mutex::new(ClientResponse::ok("{}")),
        }
    }

    /// Register the canned `GET .../events` body for a conversation id.
    #[must_use]
    pub fn with_events(self, conversation_id: &str, batch_json: &str) -> Self {
        self.events
            .lock()
            .expect("events lock")
            .push((conversation_id.to_string(), batch_json.to_string()));
        self
    }

    /// Set the canned response every POST returns.
    #[must_use]
    pub fn with_post_response(self, status: u16, body: &str) -> Self {
        *self.response.lock().expect("response lock") = ClientResponse {
            status,
            body: body.to_string(),
        };
        self
    }

    /// The POSTs this fake recorded, in order.
    #[must_use]
    pub fn recorded_posts(&self) -> Vec<RecordedPost> {
        self.posts.lock().expect("posts lock").clone()
    }
}

impl InteractionClient for FakeInteractionClient {
    fn fetch_events(&self, conversation_id: &str) -> Result<String, String> {
        self.events
            .lock()
            .expect("events lock")
            .iter()
            .find(|(id, _)| id == conversation_id)
            .map(|(_, body)| body.clone())
            .ok_or_else(|| format!("no canned events for {conversation_id}"))
    }

    fn post_turn(&self, conversation_id: &str, body: &str) -> Result<ClientResponse, String> {
        self.posts
            .lock()
            .expect("posts lock")
            .push(RecordedPost::Turn {
                conversation_id: conversation_id.to_string(),
                body: body.to_string(),
            });
        Ok(self.response.lock().expect("response lock").clone())
    }

    fn post_accept(
        &self,
        conversation_id: &str,
        proposal_id: &str,
    ) -> Result<ClientResponse, String> {
        self.posts
            .lock()
            .expect("posts lock")
            .push(RecordedPost::Accept {
                conversation_id: conversation_id.to_string(),
                proposal_id: proposal_id.to_string(),
            });
        Ok(self.response.lock().expect("response lock").clone())
    }

    fn post_new_conversation(&self, body: &str) -> Result<ClientResponse, String> {
        self.posts
            .lock()
            .expect("posts lock")
            .push(RecordedPost::NewConversation {
                body: body.to_string(),
            });
        Ok(self.response.lock().expect("response lock").clone())
    }

    fn tracked_conversations(&self) -> Vec<String> {
        self.events
            .lock()
            .expect("events lock")
            .iter()
            .map(|(id, _)| id.clone())
            .collect()
    }
}
