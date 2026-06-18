// SPDX-License-Identifier: MPL-2.0

use std::sync::Arc;

use super::*;
use crate::conversation::proxy::ConversationProxy;
use crate::conversation::test_support::FakeInteractionClient;

const FIXTURE: &str = include_str!("../../ui/fixtures/conversation-events.json");

#[test]
fn poll_tick_drains_all_tracked_conversations_to_the_hub() {
    let client = FakeInteractionClient::new()
        .with_events("conversation-1", FIXTURE)
        .with_events("conversation-2", FIXTURE);
    let proxy = Arc::new(ConversationProxy::new(Arc::new(client)));
    let sub = proxy.broadcaster().subscribe();

    poll_tick(&proxy);

    // Both conversations' four events fan out on the single stream (8 frames),
    // each carrying its conversation_id so the client demultiplexes.
    let mut received = Vec::new();
    for _ in 0..8 {
        received.push(sub.recv().expect("frame"));
    }
    assert_eq!(received.len(), 8);
    assert!(received.iter().all(|f| f.starts_with("data: ")));
}

#[test]
fn second_tick_is_quiet_after_cursor_caught_up() {
    let client = FakeInteractionClient::new().with_events("conversation-1", FIXTURE);
    let proxy = Arc::new(ConversationProxy::new(Arc::new(client)));
    let sub = proxy.broadcaster().subscribe();

    poll_tick(&proxy); // emits 4
    for _ in 0..4 {
        sub.recv().expect("frame");
    }

    poll_tick(&proxy); // same batch, cursor caught up => no frames
    assert!(
        sub.try_recv().is_err(),
        "a re-poll from the cursor must not re-emit events"
    );
    assert_eq!(proxy.cursor("conversation-1"), 4);
}
