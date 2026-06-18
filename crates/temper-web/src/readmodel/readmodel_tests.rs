// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::board::{ActivityKind, Sev};

fn card(id: &str, lane: Lane) -> Card {
    Card {
        id: id.to_string(),
        lane,
        artifact_ref: format!("acme/widgets#{}", id.trim_start_matches('i')),
        role: "code".to_string(),
        title: format!("title {id}"),
        entered_at: 0,
        steps: None,
        activity: None,
        ci: None,
        merged: None,
    }
}

#[test]
fn empty_model_snapshot_is_seq_zero() {
    let model = ReadModel::new();
    match model.snapshot_event() {
        BoardEvent::Snapshot { seq, state } => {
            assert_eq!(seq, 0);
            assert!(state.cards.is_empty());
        }
        other => panic!("expected snapshot, got {other:?}"),
    }
}

#[test]
fn move_card_updates_lane_and_resets_age_and_bumps_seq() {
    let mut model = ReadModel::new();
    model.apply(Delta::UpsertCard(card("i42", Lane::Triage)));
    let before = model.seq();
    let event = model
        .apply(Delta::MoveCard {
            id: "i42".to_string(),
            lane: Lane::Implement,
            now: 1700,
        })
        .expect("emits");
    assert_eq!(model.seq(), before + 1);
    assert_eq!(model.cards()["i42"].lane, Lane::Implement);
    assert_eq!(model.cards()["i42"].entered_at, 1700);
    match event {
        BoardEvent::CardMove { id, lane, now, .. } => {
            assert_eq!(id, "i42");
            assert_eq!(lane, Lane::Implement);
            assert_eq!(now, 1700);
        }
        other => panic!("expected card.move, got {other:?}"),
    }
}

#[test]
fn move_unknown_card_still_emits_and_advances_cursor() {
    // Mirrors the TS reducer: an unknown-id delta is a no-op on state but still
    // advances the shared seq cursor so the two sides stay aligned.
    let mut model = ReadModel::new();
    let before = model.seq();
    let event = model
        .apply(Delta::MoveCard {
            id: "ghost".to_string(),
            lane: Lane::Done,
            now: 5,
        })
        .expect("emits even for unknown id");
    assert_eq!(model.seq(), before + 1);
    assert!(model.cards().is_empty());
    assert!(matches!(event, BoardEvent::CardMove { .. }));
}

#[test]
fn upsert_identical_card_is_noop() {
    let mut model = ReadModel::new();
    model.apply(Delta::UpsertCard(card("i42", Lane::Triage)));
    let seq = model.seq();
    let event = model.apply(Delta::UpsertCard(card("i42", Lane::Triage)));
    assert!(event.is_none());
    assert_eq!(model.seq(), seq);
}

#[test]
fn add_and_clear_problem() {
    let mut model = ReadModel::new();
    let problem = Problem {
        sev: Sev::Bad,
        msg: "PR#6 CI failed".to_string(),
        card: "p6".to_string(),
        since: 0,
    };
    let added = model
        .apply(Delta::AddProblem {
            id: "p6-ci".to_string(),
            problem: problem.clone(),
        })
        .expect("emits");
    assert!(matches!(added, BoardEvent::ProblemAdd { .. }));
    assert_eq!(model.problems().len(), 1);

    // re-adding the same problem is a no-op
    assert!(
        model
            .apply(Delta::AddProblem {
                id: "p6-ci".to_string(),
                problem,
            })
            .is_none()
    );

    let cleared = model
        .apply(Delta::ClearProblem {
            id: "p6-ci".to_string(),
        })
        .expect("emits");
    assert!(matches!(cleared, BoardEvent::ProblemClear { .. }));
    assert!(model.problems().is_empty());

    // clearing a missing problem is a no-op
    assert!(
        model
            .apply(Delta::ClearProblem {
                id: "p6-ci".to_string()
            })
            .is_none()
    );
}

#[test]
fn set_activity_and_steps_update_card() {
    let mut model = ReadModel::new();
    model.apply(Delta::UpsertCard(card("i39", Lane::Implement)));
    model.apply(Delta::SetSteps {
        id: "i39".to_string(),
        steps: Steps { done: 3, total: 4 },
    });
    model.apply(Delta::SetActivity {
        id: "i39".to_string(),
        activity: Activity {
            kind: ActivityKind::Tool,
            text: "cargo test".to_string(),
        },
    });
    let c = &model.cards()["i39"];
    assert_eq!(c.steps, Some(Steps { done: 3, total: 4 }));
    assert_eq!(
        c.activity.as_ref().map(|a| a.text.as_str()),
        Some("cargo test")
    );
}

#[test]
fn set_merged_clears_ci_and_marks_done() {
    let mut model = ReadModel::new();
    model.apply(Delta::UpsertCard(card("p6", Lane::Ci)));
    model.apply(Delta::SetCi {
        id: "p6".to_string(),
        ci: Some(CiStatus::Failed),
    });
    assert_eq!(model.cards()["p6"].ci, Some(CiStatus::Failed));
    model.apply(Delta::SetMerged {
        id: "p6".to_string(),
        merged: true,
    });
    assert_eq!(model.cards()["p6"].merged, Some(true));
    assert_eq!(model.cards()["p6"].ci, None);
}

#[test]
fn set_workers_replaces_tile_only_on_change() {
    let mut model = ReadModel::new();
    let event = model.apply(Delta::SetWorkers(Workers {
        healthy: 2,
        total: 3,
    }));
    assert!(event.is_some());
    assert_eq!(model.workers().healthy, 2);
    // no-op when unchanged
    assert!(
        model
            .apply(Delta::SetWorkers(Workers {
                healthy: 2,
                total: 3
            }))
            .is_none()
    );
}

#[test]
fn load_snapshot_rebuilds_projection() {
    let mut model = ReadModel::new();
    model.apply(Delta::UpsertCard(card("stale", Lane::Triage)));
    let mut cards = std::collections::BTreeMap::new();
    cards.insert("fresh".to_string(), card("fresh", Lane::Review));
    model.load_snapshot(SnapshotState {
        workers: Workers {
            healthy: 1,
            total: 1,
        },
        cards,
        problems: std::collections::BTreeMap::new(),
    });
    assert!(model.cards().contains_key("fresh"));
    assert!(!model.cards().contains_key("stale"));
}

/// Build a `SnapshotState` from a list of cards (no problems, default workers).
fn snapshot_of(cards: &[Card]) -> SnapshotState {
    let mut map = std::collections::BTreeMap::new();
    for c in cards {
        map.insert(c.id.clone(), c.clone());
    }
    SnapshotState {
        workers: Workers {
            healthy: 1,
            total: 1,
        },
        cards: map,
        problems: std::collections::BTreeMap::new(),
    }
}

#[test]
fn reconcile_adds_and_drops_cards() {
    // Snapshot A: {keep, gone}. Snapshot B (well past the grace window): {keep,
    // added}. Expect `gone` dropped and `added` added; `keep` untouched.
    let mut model = ReadModel::new();
    model.load_snapshot(snapshot_of(&[
        card("keep", Lane::Triage),
        card("gone", Lane::Triage),
    ]));

    let now = ReadModel::RECONCILE_GRACE_MS + 1_000;
    let events = model.reconcile_snapshot(
        snapshot_of(&[card("keep", Lane::Triage), card("added", Lane::Triage)]),
        now,
    );

    assert!(model.cards().contains_key("keep"), "existing card kept");
    assert!(model.cards().contains_key("added"), "new card added");
    assert!(
        !model.cards().contains_key("gone"),
        "absent card dropped past grace"
    );
    // Each emitted event is a real, sequenced board event (snapshot for the
    // add/drop; the worker tile is unchanged here so no extra event).
    assert!(!events.is_empty(), "add + drop emit events");
    let mut last = 0;
    for event in &events {
        assert!(event.seq() > last, "events carry a strictly-increasing seq");
        last = event.seq();
        assert!(matches!(event, BoardEvent::Snapshot { .. }));
    }
}

#[test]
fn reconcile_grace_keeps_recently_touched_absent_card() {
    // A card the log just touched (via a delta) but the daemon no longer lists
    // must survive the reconcile while still inside the grace window.
    let mut model = ReadModel::new();
    model.apply(Delta::UpsertCard(card("busy", Lane::Implement)));
    // Touch it "now"; reconcile at the same instant — well inside the grace window.
    let now = 10_000;
    model.apply(Delta::MoveCard {
        id: "busy".to_string(),
        lane: Lane::Review,
        now,
    });

    let events = model.reconcile_snapshot(snapshot_of(&[]), now + 100);
    assert!(
        model.cards().contains_key("busy"),
        "recently-touched card kept"
    );
    // No drop event for `busy`; only the worker-tile refresh may fire.
    assert!(
        events
            .iter()
            .all(|e| matches!(e, BoardEvent::Snapshot { .. })),
        "only the worker refresh (a snapshot) may fire, never a card drop"
    );
}

#[test]
fn reconcile_never_drops_merged_or_done_cards() {
    // Merged/done cards are terminal — never auto-dropped, even long past grace.
    let mut model = ReadModel::new();
    model.apply(Delta::UpsertCard(card("pr", Lane::Ci)));
    model.apply(Delta::SetMerged {
        id: "pr".to_string(),
        merged: true,
    });
    model.apply(Delta::UpsertCard(card("shipped", Lane::Done)));

    let now = 1_000_000; // far past any grace window
    model.reconcile_snapshot(snapshot_of(&[]), now);
    assert!(
        model.cards().contains_key("pr"),
        "merged card never auto-dropped"
    );
    assert!(
        model.cards().contains_key("shipped"),
        "done-lane card never auto-dropped"
    );
}

#[test]
fn reconcile_does_not_clobber_existing_card_lifecycle() {
    // A re-poll that re-lists an existing card must NOT reset its log-refined
    // lane/activity. The snapshot would seed it in `implement`; the log moved it
    // to `review` with an activity — both must survive.
    let mut model = ReadModel::new();
    model.apply(Delta::UpsertCard(card("c", Lane::Implement)));
    model.apply(Delta::MoveCard {
        id: "c".to_string(),
        lane: Lane::Review,
        now: 500,
    });
    model.apply(Delta::SetActivity {
        id: "c".to_string(),
        activity: Activity {
            kind: ActivityKind::Think,
            text: "reviewing".to_string(),
        },
    });

    // The fresh snapshot lists the same card but seeded in `implement` (cold-split).
    model.reconcile_snapshot(snapshot_of(&[card("c", Lane::Implement)]), 600);

    let c = &model.cards()["c"];
    assert_eq!(
        c.lane,
        Lane::Review,
        "log-refined lane survives the re-poll"
    );
    assert_eq!(
        c.activity.as_ref().map(|a| a.text.as_str()),
        Some("reviewing"),
        "log-refined activity survives the re-poll"
    );
}

#[test]
fn reconcile_refreshes_worker_tile_and_saturation_problems() {
    let mut model = ReadModel::new();
    model.load_snapshot(snapshot_of(&[]));

    let mut fresh = snapshot_of(&[]);
    fresh.workers = Workers {
        healthy: 3,
        total: 4,
    };
    fresh.problems.insert(
        "sat:code:acme/widgets#7".to_string(),
        Problem {
            sev: Sev::Warn,
            msg: "code saturated".to_string(),
            card: "acmewidgets-7".to_string(),
            since: 100,
        },
    );
    model.reconcile_snapshot(fresh, 100);
    assert_eq!(
        model.workers(),
        Workers {
            healthy: 3,
            total: 4
        }
    );
    assert!(model.problems().contains_key("sat:code:acme/widgets#7"));

    // A later snapshot without that saturation clears the stale `sat:` problem.
    model.reconcile_snapshot(snapshot_of(&[]), 200);
    assert!(
        !model.problems().contains_key("sat:code:acme/widgets#7"),
        "stale saturation problem cleared"
    );
}

#[test]
fn seq_is_strictly_monotonic_across_mixed_deltas() {
    let mut model = ReadModel::new();
    let mut last = model.seq();
    model.apply(Delta::UpsertCard(card("i1", Lane::Triage)));
    for delta in [
        Delta::MoveCard {
            id: "i1".to_string(),
            lane: Lane::Implement,
            now: 1,
        },
        Delta::SetSteps {
            id: "i1".to_string(),
            steps: Steps { done: 1, total: 2 },
        },
        Delta::AddProblem {
            id: "x".to_string(),
            problem: Problem {
                sev: Sev::Warn,
                msg: "m".to_string(),
                card: "i1".to_string(),
                since: 0,
            },
        },
    ] {
        if model.apply(delta).is_some() {
            assert!(model.seq() > last, "seq must strictly increase");
            last = model.seq();
        }
    }
}
