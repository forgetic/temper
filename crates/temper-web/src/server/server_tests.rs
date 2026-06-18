// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::board::Lane;
use crate::feeds::snapshot_source::{EmptySnapshotSource, FixtureSnapshotSource};
use crate::readmodel::Delta;

const RAW_SNAPSHOT: &str = r#"{
  "workers": { "healthy": 1, "total": 1 },
  "queued":   [{ "job_id": "j42", "role": "code", "repo": "acme/widgets", "ref": "acme/widgets#42" }],
  "in_flight":[],
  "role_saturation": []
}"#;

fn empty_state() -> AppState {
    AppState::new(
        &EmptySnapshotSource,
        &LaneMap::empty(),
        0,
        std::path::PathBuf::from("/nonexistent-ui"),
    )
}

#[test]
fn healthz_route_returns_ok() {
    let state = empty_state();
    let response = route(&state, "GET", "/healthz");
    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"ok");
}

#[test]
fn api_state_returns_board_snapshot_envelope() {
    let state = AppState::new(
        &FixtureSnapshotSource::new(RAW_SNAPSHOT),
        &LaneMap::empty(),
        1000,
        std::path::PathBuf::from("/nonexistent-ui"),
    );
    let response = route(&state, "GET", "/api/state");
    assert_eq!(response.status, 200);
    assert_eq!(response.content_type, "application/json");
    let event: BoardEvent = serde_json::from_slice(&response.body).expect("parses");
    match event {
        BoardEvent::Snapshot { state, .. } => {
            assert_eq!(state.workers.total, 1);
            assert_eq!(state.cards.len(), 1);
            let card = state.cards.values().next().unwrap();
            assert_eq!(card.lane, Lane::Triage);
        }
        other => panic!("expected snapshot, got {other:?}"),
    }
}

#[test]
fn empty_snapshot_serves_empty_board() {
    let state = empty_state();
    let response = route(&state, "GET", "/api/state");
    let event: BoardEvent = serde_json::from_slice(&response.body).expect("parses");
    match event {
        BoardEvent::Snapshot { seq, state } => {
            assert_eq!(seq, 0);
            assert!(state.cards.is_empty());
        }
        other => panic!("expected snapshot, got {other:?}"),
    }
}

#[test]
fn unknown_route_is_404() {
    let state = empty_state();
    assert_eq!(route(&state, "GET", "/missing.js").status, 404);
    assert_eq!(route(&state, "POST", "/api/state").status, 404);
}

#[test]
fn ingest_applies_delta_and_fans_out_to_subscribers() {
    let state = AppState::new(
        &FixtureSnapshotSource::new(RAW_SNAPSHOT),
        &LaneMap::empty(),
        0,
        std::path::PathBuf::from("/nonexistent-ui"),
    );
    let sub = state.broadcaster().subscribe();
    let card_id = crate::project::snapshot::card_id_for_ref("acme/widgets#42");
    state.ingest(Delta::MoveCard {
        id: card_id.clone(),
        lane: Lane::Implement,
        now: 5,
    });
    let frame = sub.recv().expect("a frame");
    assert!(frame.starts_with("data: "));
    let json = frame.trim_start_matches("data: ").trim_end();
    let event: BoardEvent = serde_json::from_str(json).expect("parses");
    match event {
        BoardEvent::CardMove { id, lane, .. } => {
            assert_eq!(id, card_id);
            assert_eq!(lane, Lane::Implement);
        }
        other => panic!("expected card.move, got {other:?}"),
    }
}

#[test]
fn snapshot_cursor_advances_after_ingest_so_resume_has_no_gap() {
    let state = empty_state();
    let before = state.snapshot_event().seq();
    state.ingest(Delta::SetWorkers(crate::board::Workers {
        healthy: 2,
        total: 2,
    }));
    let after = state.snapshot_event().seq();
    assert!(after > before, "cursor must advance so SSE resumes cleanly");
}

// --- Conversation proxy routing (feed 2) ---

use crate::conversation::ConversationProxy;
use crate::conversation::test_support::{FakeInteractionClient, RecordedPost};

/// Build an [`AppState`] with a conversation proxy over a shared fake client, so
/// the test can both route through the server AND inspect what the fake recorded.
fn state_with_fake(
    client: FakeInteractionClient,
) -> (AppState, std::sync::Arc<FakeInteractionClient>) {
    let client = std::sync::Arc::new(client);
    let proxy = std::sync::Arc::new(ConversationProxy::new(std::sync::Arc::clone(&client) as _));
    let state = empty_state().with_conversation_proxy(proxy);
    (state, client)
}

#[test]
fn conversation_post_is_503_when_proxy_disabled() {
    let state = empty_state();
    let response = route_conversation_post(&state, "/conversations/c1/turns", b"{}");
    assert_eq!(response.status, 503);
}

#[test]
fn turn_post_forwards_body_and_passes_response_through() {
    let client =
        FakeInteractionClient::new().with_post_response(200, r#"{"reply":{"message":"ok"}}"#);
    let (state, fake) = state_with_fake(client);

    let response = route_conversation_post(
        &state,
        "/conversations/conversation-1/turns",
        br#"{"body":"hi"}"#,
    );

    assert_eq!(response.status, 200);
    assert_eq!(response.content_type, "application/json");
    assert_eq!(response.body, br#"{"reply":{"message":"ok"}}"#);
    assert_eq!(
        fake.recorded_posts(),
        vec![RecordedPost::Turn {
            conversation_id: "conversation-1".to_string(),
            body: r#"{"body":"hi"}"#.to_string(),
        }]
    );
}

#[test]
fn accept_post_forwards_both_ids() {
    let client = FakeInteractionClient::new().with_post_response(200, "{}");
    let (state, fake) = state_with_fake(client);

    let response = route_conversation_post(
        &state,
        "/conversations/conversation-1/proposals/csv-export/accept",
        b"",
    );
    assert_eq!(response.status, 200);
    assert_eq!(
        fake.recorded_posts(),
        vec![RecordedPost::Accept {
            conversation_id: "conversation-1".to_string(),
            proposal_id: "csv-export".to_string(),
        }]
    );
}

#[test]
fn new_conversation_post_forwards_and_preserves_201() {
    let client =
        FakeInteractionClient::new().with_post_response(201, r#"{"conversation_id":"c2"}"#);
    let (state, fake) = state_with_fake(client);

    let response = route_conversation_post(&state, "/conversations", b"{}");
    assert_eq!(response.status, 201, "upstream status is preserved");
    assert_eq!(response.body, br#"{"conversation_id":"c2"}"#);
    assert_eq!(
        fake.recorded_posts(),
        vec![RecordedPost::NewConversation {
            body: "{}".to_string()
        }]
    );
}

#[test]
fn unknown_conversation_subpath_is_404() {
    let client = FakeInteractionClient::new();
    let (state, _fake) = state_with_fake(client);
    assert_eq!(
        route_conversation_post(&state, "/conversations/c1/bogus", b"{}").status,
        404
    );
}
