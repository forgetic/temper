// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::board::{Card, Lane};
use crate::readmodel::ReadModel;

fn adapter() -> LogTailAdapter {
    LogTailAdapter::new(LaneMap::empty(), 1000)
}

const TRANSITION: &str = include_str!("../../ui/fixtures/events/transition-applied.json");
const CI_FAILURE: &str = include_str!("../../ui/fixtures/events/ci-completed-failure.json");
const PR_MERGED: &str = include_str!("../../ui/fixtures/events/pr-merged.json");
const LEASE_LOST: &str = include_str!("../../ui/fixtures/events/lease-lost.json");
const ROLE_SATURATED: &str = include_str!("../../ui/fixtures/events/role-saturated.json");
const AGENT_STARTED: &str = include_str!("../../ui/fixtures/events/agent-started.json");

#[test]
fn transition_applied_moves_card_by_added_label() {
    // labels.delta "-untriaged +code +ready" -> the +code lifecycle token,
    // which (no lifecycle dimension here) name-matches to the implement lane.
    let deltas = adapter().deltas_for_line(TRANSITION);
    assert_eq!(
        deltas,
        vec![Delta::MoveCard {
            id: card_id_for_ref("acme/widgets#42"),
            lane: Lane::Implement,
            now: 1000,
        }]
    );
}

#[test]
fn ci_failure_sets_badge_and_problem() {
    let deltas = adapter().deltas_for_line(CI_FAILURE);
    let card = card_id_for_ref("acme/widgets PR#6");
    assert_eq!(
        deltas,
        vec![
            Delta::SetCi {
                id: card.clone(),
                ci: Some(CiStatus::Failed),
            },
            Delta::AddProblem {
                id: format!("ci:{card}"),
                problem: Problem {
                    sev: Sev::Bad,
                    msg: "acme/widgets PR#6 CI failed".to_string(),
                    card,
                    since: 1000,
                },
            },
        ]
    );
}

#[test]
fn pr_merged_lands_done_and_clears_ci_problem() {
    let deltas = adapter().deltas_for_line(PR_MERGED);
    let card = card_id_for_ref("acme/widgets PR#6");
    assert_eq!(
        deltas,
        vec![
            Delta::MoveCard {
                id: card.clone(),
                lane: Lane::Done,
                now: 1000,
            },
            Delta::SetMerged {
                id: card.clone(),
                merged: true,
            },
            Delta::ClearProblem {
                id: format!("ci:{card}"),
            },
        ]
    );
}

#[test]
fn lease_lost_raises_attention_problem() {
    let deltas = adapter().deltas_for_line(LEASE_LOST);
    let card = card_id_for_ref("acme/widgets PR#8");
    assert_eq!(deltas.len(), 1);
    match &deltas[0] {
        Delta::AddProblem { id, problem } => {
            assert_eq!(id, &format!("lease:{card}"));
            assert_eq!(problem.sev, Sev::Bad);
            assert!(problem.msg.contains("lease lost"));
            assert!(problem.msg.contains("expired (held 11m)"));
        }
        other => panic!("expected problem, got {other:?}"),
    }
}

#[test]
fn role_saturated_makes_overlay_problems_not_lanes() {
    let deltas = adapter().deltas_for_line(ROLE_SATURATED);
    // one problem per queued ref, none are card moves (queues are overlays, A.4)
    assert_eq!(deltas.len(), 2);
    assert!(deltas.iter().all(|d| matches!(d, Delta::AddProblem { .. })));
    match &deltas[0] {
        Delta::AddProblem { id, problem } => {
            assert_eq!(id, "sat:code:acme/api#7");
            assert_eq!(problem.sev, Sev::Warn);
            assert_eq!(problem.card, card_id_for_ref("acme/api#7"));
            assert!(problem.msg.contains("code saturated"));
        }
        other => panic!("expected problem, got {other:?}"),
    }
}

#[test]
fn agent_started_clears_lease_and_sets_activity() {
    let deltas = adapter().deltas_for_line(AGENT_STARTED);
    let card = card_id_for_ref("acme/widgets#39");
    assert_eq!(
        deltas,
        vec![
            Delta::ClearProblem {
                id: format!("lease:{card}"),
            },
            Delta::SetActivity {
                id: card,
                activity: Activity {
                    kind: ActivityKind::Think,
                    text: "code: coding".to_string(),
                },
            },
        ]
    );
}

#[test]
fn unparseable_line_yields_no_deltas() {
    assert!(adapter().deltas_for_line("not json").is_empty());
    assert!(adapter().deltas_for_line("{}").is_empty());
    // a known-shaped line with an out-of-vocabulary event is ignored
    assert!(
        adapter()
            .deltas_for_line(r#"{"fields":{"event":"queue.entered","artifact.ref":"a/b#1"}}"#)
            .is_empty()
    );
}

#[test]
fn drain_replays_a_canned_line_stream_in_order() {
    let lines = vec![
        TRANSITION.to_string(),
        AGENT_STARTED.to_string(),
        CI_FAILURE.to_string(),
        PR_MERGED.to_string(),
    ];
    let mut source = lines.into_iter();
    let deltas = adapter().drain(&mut source);
    // transition(1) + agent.started(2) + ci.failure(2) + pr.merged(3)
    assert_eq!(deltas.len(), 8);
}

#[test]
fn end_to_end_fixture_replay_into_readmodel() {
    // The cross-side contract: feed the same fixtures the TS apply() consumes,
    // through the adapter into the read-model, and assert the projected board.
    let mut model = ReadModel::new();
    // seed the cards the deltas join onto (as a snapshot would)
    for r in ["acme/widgets#42", "acme/widgets PR#6", "acme/widgets#39"] {
        model.apply(Delta::UpsertCard(Card {
            id: card_id_for_ref(r),
            lane: Lane::Triage,
            artifact_ref: r.to_string(),
            role: "code".to_string(),
            title: r.to_string(),
            entered_at: 0,
            steps: None,
            activity: None,
            ci: None,
            merged: None,
        }));
    }
    let adapter = adapter();
    for line in [TRANSITION, AGENT_STARTED, CI_FAILURE] {
        for delta in adapter.deltas_for_line(line) {
            model.apply(delta);
        }
    }
    // i42 moved to implement; PR#6 has a failed-CI badge + problem; #39 active.
    assert_eq!(
        model.cards()[&card_id_for_ref("acme/widgets#42")].lane,
        Lane::Implement
    );
    assert_eq!(
        model.cards()[&card_id_for_ref("acme/widgets PR#6")].ci,
        Some(CiStatus::Failed)
    );
    assert!(
        model
            .problems()
            .contains_key(&format!("ci:{}", card_id_for_ref("acme/widgets PR#6")))
    );
    assert!(
        model.cards()[&card_id_for_ref("acme/widgets#39")]
            .activity
            .is_some()
    );
}
