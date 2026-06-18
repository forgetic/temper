// SPDX-License-Identifier: MPL-2.0

//! The background conversation poller.
//!
//! Mirrors PR D's `pump_log_source`: a thread that, on an interval, drains every
//! tracked conversation's event batch through the proxy (which advances cursors
//! and broadcasts new events as SSE frames) and emits a keep-alive when nothing
//! moved, so an idle-but-live conversation pipe is distinguishable from a dead one
//! (UX §4.2 / §6.3). The proxy reuses PR D's `Broadcaster` for the actual fan-out.

use std::sync::Arc;
use std::time::Duration;

use super::proxy::ConversationProxy;

/// Run one poll tick over all tracked conversations, returning the number of SSE
/// subscribers that were live before the tick (for keep-alive accounting). Pulled
/// out of the loop so a test can drive a single deterministic tick.
pub fn poll_tick(proxy: &ConversationProxy) {
    for conversation_id in proxy.tracked_conversations() {
        proxy.poll_and_broadcast(&conversation_id);
    }
}

/// Spawn the conversation poll loop on a background thread. Every `interval` it
/// runs a [`poll_tick`]; the proxy broadcasts new events, and on a quiet tick we
/// send a keep-alive so the chat page's pulse can tell idle from dead.
pub fn spawn_poller(proxy: Arc<ConversationProxy>, interval: Duration) {
    std::thread::spawn(move || {
        loop {
            poll_tick(&proxy);
            // Keep the pipe provably alive even when no conversation moved.
            proxy
                .broadcaster()
                .broadcast(&crate::server::sse::keep_alive());
            std::thread::sleep(interval);
        }
    });
}

#[cfg(test)]
#[path = "poller_tests.rs"]
mod poller_tests;
