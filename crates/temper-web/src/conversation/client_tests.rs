// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::conversation::test_support::{FakeInteractionClient, RecordedPost};

#[test]
fn client_response_ok_defaults_to_200() {
    let response = ClientResponse::ok(r#"{"x":1}"#);
    assert_eq!(response.status, 200);
    assert_eq!(response.body, r#"{"x":1}"#);
}

#[test]
fn fake_client_records_turn_with_exact_body() {
    let client = FakeInteractionClient::new();
    let response = client
        .post_turn("conversation-1", r#"{"body":"hi"}"#)
        .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(
        client.recorded_posts(),
        vec![RecordedPost::Turn {
            conversation_id: "conversation-1".to_string(),
            body: r#"{"body":"hi"}"#.to_string(),
        }]
    );
}

#[test]
fn fake_client_tracks_only_seeded_conversations() {
    let client = FakeInteractionClient::new().with_events("c1", "[]");
    assert_eq!(client.tracked_conversations(), vec!["c1".to_string()]);
    assert!(client.fetch_events("c2").is_err());
}
