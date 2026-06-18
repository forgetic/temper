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
