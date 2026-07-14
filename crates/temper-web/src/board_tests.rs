// SPDX-License-Identifier: MPL-2.0

use super::*;
use serde_json::json;

#[test]
fn snapshot_serializes_to_board_envelope() {
    let mut cards = std::collections::BTreeMap::new();
    cards.insert(
        "i42".to_string(),
        Card {
            id: "i42".to_string(),
            lane: Lane::Triage,
            artifact_ref: "widgets#42".to_string(),
            role: "code".to_string(),
            title: "Add CSV export".to_string(),
            entered_at: 0,
            steps: None,
            activity: None,
            ci: None,
            merged: None,
        },
    );
    let event = BoardEvent::Snapshot {
        seq: 100,
        state: SnapshotState {
            workers: Workers {
                healthy: 3,
                total: 3,
            },
            cards,
            problems: std::collections::BTreeMap::new(),
        },
    };
    let value = serde_json::to_value(&event).expect("serializes");
    assert_eq!(
        value,
        json!({
            "t": "snapshot",
            "seq": 100,
            "state": {
                "workers": { "healthy": 3, "total": 3 },
                "cards": {
                    "i42": {
                        "id": "i42",
                        "lane": "triage",
                        "ref": "widgets#42",
                        "role": "code",
                        "title": "Add CSV export",
                        "enteredAt": 0
                    }
                },
                "problems": {}
            }
        })
    );
}

#[test]
fn card_with_all_fields_uses_camelcase_keys() {
    let card = Card {
        id: "i39".to_string(),
        lane: Lane::Implement,
        artifact_ref: "widgets#39".to_string(),
        role: "code".to_string(),
        title: "Refactor feed mapper".to_string(),
        entered_at: 0,
        steps: Some(Steps { done: 3, total: 4 }),
        activity: Some(Activity {
            kind: ActivityKind::Tool,
            text: "cargo test".to_string(),
        }),
        ci: None,
        merged: None,
    };
    let value = serde_json::to_value(&card).expect("serializes");
    assert_eq!(
        value,
        json!({
            "id": "i39",
            "lane": "implement",
            "ref": "widgets#39",
            "role": "code",
            "title": "Refactor feed mapper",
            "enteredAt": 0,
            "steps": { "done": 3, "total": 4 },
            "activity": { "kind": "tool", "text": "cargo test" }
        })
    );
}

#[test]
fn card_move_event_matches_ts_shape() {
    let event = BoardEvent::CardMove {
        seq: 101,
        id: "i42".to_string(),
        lane: Lane::Implement,
        now: 1700,
    };
    let value = serde_json::to_value(&event).expect("serializes");
    assert_eq!(
        value,
        json!({ "t": "card.move", "seq": 101, "id": "i42", "lane": "implement", "now": 1700 })
    );
}

#[test]
fn problem_add_event_matches_ts_shape() {
    let event = BoardEvent::ProblemAdd {
        seq: 102,
        id: "p6-ci".to_string(),
        problem: Problem {
            sev: Sev::Bad,
            msg: "PR#6 CI failed".to_string(),
            card: "p6".to_string(),
            since: 0,
        },
    };
    let value = serde_json::to_value(&event).expect("serializes");
    assert_eq!(
        value,
        json!({
            "t": "problem.add",
            "seq": 102,
            "id": "p6-ci",
            "problem": { "sev": "bad", "msg": "PR#6 CI failed", "card": "p6", "since": 0 }
        })
    );
}

#[test]
fn problem_clear_and_card_step_shapes() {
    let clear = serde_json::to_value(BoardEvent::ProblemClear {
        seq: 103,
        id: "p6-ci".to_string(),
    })
    .expect("serializes");
    assert_eq!(
        clear,
        json!({ "t": "problem.clear", "seq": 103, "id": "p6-ci" })
    );

    let step = serde_json::to_value(BoardEvent::CardStep {
        seq: 104,
        id: "i39".to_string(),
        steps: Steps { done: 4, total: 4 },
    })
    .expect("serializes");
    assert_eq!(
        step,
        json!({ "t": "card.step", "seq": 104, "id": "i39", "steps": { "done": 4, "total": 4 } })
    );
}

#[test]
fn card_stream_event_matches_existing_typescript_shape() {
    let event = BoardEvent::CardStream {
        seq: 105,
        id: "i39".to_string(),
        event: StreamEvent {
            kind: StreamEventKind::Tool,
            k: Some("bash".to_string()),
            v: "finished (42 ms)".to_string(),
        },
    };
    assert_eq!(
        serde_json::to_value(&event).expect("serializes"),
        json!({
            "t": "card.stream",
            "seq": 105,
            "id": "i39",
            "event": { "kind": "tool", "k": "bash", "v": "finished (42 ms)" }
        })
    );
    assert_eq!(event.seq(), 105);
}

#[test]
fn shared_typescript_stream_fixture_parses_as_rust_board_events() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../ui/fixtures/activity/stream-i41.json"))
            .expect("fixture JSON");
    let events = fixture["events"].as_array().expect("events array");
    assert!(!events.is_empty());
    for value in events {
        let event: BoardEvent = serde_json::from_value(value.clone()).expect("Rust contract");
        assert!(matches!(event, BoardEvent::CardStream { .. }));
    }
}

#[test]
fn fixture_snapshot_parses_into_board_types() {
    // The shared cold-start contract fixture must round-trip through our types,
    // proving the Rust projection emits exactly what the TS reducer reads.
    let raw = include_str!("../ui/fixtures/state-snapshot.json");
    let event: BoardEvent = serde_json::from_str(raw).expect("fixture parses");
    match event {
        BoardEvent::Snapshot { seq, state } => {
            assert_eq!(seq, 100);
            assert_eq!(state.workers.healthy, 3);
            assert_eq!(state.cards.len(), 4);
            assert_eq!(state.cards["p6"].ci, Some(CiStatus::Failed));
            assert_eq!(state.problems.len(), 1);
            assert_eq!(state.problems["p6-ci"].sev, Sev::Bad);
        }
        other => panic!("expected snapshot, got {other:?}"),
    }
}

#[test]
fn board_event_seq_accessor() {
    assert_eq!(
        BoardEvent::ProblemClear {
            seq: 7,
            id: "x".to_string()
        }
        .seq(),
        7
    );
}
