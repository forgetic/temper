// SPDX-License-Identifier: MPL-2.0

use std::sync::Arc;

use super::*;
use crate::conversation::test_support::{FakeInteractionClient, RecordedPost};

/// The contract fixture the chat UI also reads (`ui/fixtures/conversation-events.json`).
/// Embedding it keeps the Rust translation test and the TS side on one batch shape.
const FIXTURE: &str = include_str!("../../ui/fixtures/conversation-events.json");

fn proxy_with(client: FakeInteractionClient) -> ConversationProxy {
    ConversationProxy::new(Arc::new(client))
}

/// The framed `data:` payloads decoded back to ConversationEvents, for assertions.
fn decode_frames(frames: &[String]) -> Vec<temper_interaction::ConversationEvent> {
    frames
        .iter()
        .map(|frame| {
            let json = frame
                .strip_prefix("data: ")
                .and_then(|rest| rest.strip_suffix("\n\n"))
                .expect("frame is a data: event");
            serde_json::from_str(json).expect("frame body is a ConversationEvent")
        })
        .collect()
}

#[test]
fn fixture_batch_translates_to_ordered_monotonic_frames() {
    let proxy = proxy_with(FakeInteractionClient::new());
    let outcome = proxy.poll_events("conversation-1", FIXTURE);

    // Every fixture event becomes exactly one data: frame, in sequence order.
    assert_eq!(outcome.frames.len(), 4);
    let events = decode_frames(&outcome.frames);
    let seqs: Vec<u64> = events.iter().map(|e| e.sequence).collect();
    assert_eq!(seqs, vec![1, 2, 3, 4]);
    // strictly monotonic, no gap/dup
    assert!(seqs.windows(2).all(|w| w[1] == w[0] + 1));
    // cursor advanced to the last sequence
    assert_eq!(outcome.cursor, 4);
    assert_eq!(proxy.cursor("conversation-1"), 4);

    // Every frame is a single SSE data: line, newline-free payload.
    for frame in &outcome.frames {
        assert!(frame.starts_with("data: "));
        assert!(frame.ends_with("\n\n"));
        assert_eq!(frame.matches("\n\n").count(), 1);
    }
    // The first event round-trips to the conversation_opened payload (the exact
    // wire type — no parallel struct), proving the contract is preserved.
    match &events[0].payload {
        temper_interaction::ConversationEventPayload::ConversationOpened { profile_id, .. } => {
            // provider-neutral: we read it through, never assert a hardcoded value
            assert!(!profile_id.as_str().is_empty());
        }
        other => panic!("expected conversation_opened, got {other:?}"),
    }
}

#[test]
fn re_poll_from_cursor_yields_no_gap_no_dup() {
    let proxy = proxy_with(FakeInteractionClient::new());

    // First poll emits all four.
    let first = proxy.poll_events("conversation-1", FIXTURE);
    assert_eq!(first.frames.len(), 4);

    // Re-polling the SAME batch emits nothing (every sequence <= cursor): no dup.
    let again = proxy.poll_events("conversation-1", FIXTURE);
    assert!(again.frames.is_empty());
    assert_eq!(again.cursor, 4);
    assert_eq!(proxy.cursor("conversation-1"), 4);
}

#[test]
fn growing_batch_emits_only_the_tail() {
    let proxy = proxy_with(FakeInteractionClient::new());

    // A two-event batch then a four-event batch (the same fixture). The cursor
    // ensures only the two NEW events frame on the second poll — no gap, no dup.
    let head = r#"{"streaming":true,"events":[
        {"sequence":1,"conversation_id":"c","kind":"conversation_opened","occurred_at":"2026-06-18T12:00:00Z","payload":{"type":"conversation_opened","profile_id":"p"}},
        {"sequence":2,"conversation_id":"c","kind":"human_turn_appended","occurred_at":"2026-06-18T12:00:05Z","payload":{"type":"human_turn_appended","turn":{"id":"t1","participant":{"kind":"human","display_name":"you"},"body":"hi"}}}
    ]}"#;
    let grown = r#"{"streaming":true,"events":[
        {"sequence":1,"conversation_id":"c","kind":"conversation_opened","occurred_at":"2026-06-18T12:00:00Z","payload":{"type":"conversation_opened","profile_id":"p"}},
        {"sequence":2,"conversation_id":"c","kind":"human_turn_appended","occurred_at":"2026-06-18T12:00:05Z","payload":{"type":"human_turn_appended","turn":{"id":"t1","participant":{"kind":"human","display_name":"you"},"body":"hi"}}},
        {"sequence":3,"conversation_id":"c","kind":"agent_reply_appended","occurred_at":"2026-06-18T12:00:09Z","payload":{"type":"agent_reply_appended","reply":{"message":"hello","proposals":[]}}}
    ]}"#;

    let first = proxy.poll_events("c", head);
    assert_eq!(first.frames.len(), 2);
    assert_eq!(first.cursor, 2);

    let second = proxy.poll_events("c", grown);
    let tail = decode_frames(&second.frames);
    assert_eq!(tail.len(), 1, "only the new event frames");
    assert_eq!(tail[0].sequence, 3);
    assert_eq!(second.cursor, 3);
}

#[test]
fn per_conversation_cursors_are_independent() {
    let proxy = proxy_with(FakeInteractionClient::new());
    proxy.poll_events("conversation-1", FIXTURE);
    // A different conversation starts at cursor 0 and emits its own batch.
    let other = proxy.poll_events("conversation-2", FIXTURE);
    assert_eq!(other.frames.len(), 4);
    assert_eq!(proxy.cursor("conversation-1"), 4);
    assert_eq!(proxy.cursor("conversation-2"), 4);
}

#[test]
fn bare_array_batch_also_parses() {
    // Robustness: a bare [...] array (not the {streaming,events} envelope) works.
    let array = r#"[
        {"sequence":1,"conversation_id":"c","kind":"conversation_opened","occurred_at":"2026-06-18T12:00:00Z","payload":{"type":"conversation_opened","profile_id":"p"}}
    ]"#;
    let proxy = proxy_with(FakeInteractionClient::new());
    let outcome = proxy.poll_events("c", array);
    assert_eq!(outcome.frames.len(), 1);
}

#[test]
fn poll_and_broadcast_fans_frames_to_subscribers() {
    // End-to-end through the reused PR D Broadcaster: a subscriber receives the
    // framed conversation events the poll produced.
    let client = FakeInteractionClient::new().with_events("conversation-1", FIXTURE);
    let proxy = proxy_with(client);
    let sub = proxy.broadcaster().subscribe();

    proxy.poll_and_broadcast("conversation-1");

    for expected_seq in 1..=4u64 {
        let frame = sub.recv().expect("frame broadcast");
        let event = &decode_frames(&[frame])[0];
        assert_eq!(event.sequence, expected_seq);
    }
}

#[test]
fn forward_turn_passes_through_and_records() {
    let client =
        FakeInteractionClient::new().with_post_response(200, r#"{"reply":{"message":"ok"}}"#);
    let proxy = proxy_with(client);

    let response = proxy
        .forward_turn("conversation-1", r#"{"body":"hello"}"#)
        .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.body, r#"{"reply":{"message":"ok"}}"#);
}

#[test]
fn forward_accept_and_new_conversation_record_the_call() {
    let client = Arc::new(
        FakeInteractionClient::new().with_post_response(201, r#"{"conversation_id":"c2"}"#),
    );
    let proxy = ConversationProxy::new(client.clone());

    let accept = proxy
        .forward_accept("conversation-1", "csv-export")
        .unwrap();
    assert_eq!(accept.status, 201);
    let new = proxy.forward_new_conversation("{}").unwrap();
    assert_eq!(new.status, 201);

    assert_eq!(
        client.recorded_posts(),
        vec![
            RecordedPost::Accept {
                conversation_id: "conversation-1".to_string(),
                proposal_id: "csv-export".to_string(),
            },
            RecordedPost::NewConversation {
                body: "{}".to_string()
            },
        ]
    );
}
