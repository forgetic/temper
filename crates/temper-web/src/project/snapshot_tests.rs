// SPDX-License-Identifier: MPL-2.0

use super::*;

const RAW_SNAPSHOT: &str = r#"{
  "workers": { "healthy": 1, "total": 1 },
  "queued":   [{ "job_id": "job-42", "role": "code", "repo": "acme/widgets",
                 "artifact": { "item": 42, "kind": "issue" }, "ref": "acme/widgets#42" }],
  "in_flight":[{ "job_id": "job-39", "role": "code", "repo": "acme/widgets",
                 "artifact": { "item": 39, "kind": "issue" }, "ref": "acme/widgets#39" }],
  "role_saturation": [{ "role": "code", "concurrency": 1, "waiting": ["acme/widgets#42"] }]
}"#;

#[test]
fn parses_documented_daemon_shape() {
    let raw: RawDaemonSnapshot = serde_json::from_str(RAW_SNAPSHOT).expect("parses");
    assert_eq!(raw.workers.healthy, 1);
    assert_eq!(raw.queued.len(), 1);
    assert_eq!(raw.in_flight.len(), 1);
    assert_eq!(
        raw.queued[0].artifact_ref.as_deref(),
        Some("acme/widgets#42")
    );
}

#[test]
fn projects_jobs_into_board_cards_with_split_lanes() {
    let raw: RawDaemonSnapshot = serde_json::from_str(RAW_SNAPSHOT).expect("parses");
    let state = project_snapshot(&raw, &LaneMap::empty(), 1000);

    assert_eq!(state.workers.healthy, 1);
    assert_eq!(state.cards.len(), 2);

    let queued = &state.cards[&card_id_for_ref("acme/widgets#42")];
    assert_eq!(queued.lane, Lane::Triage); // queued -> triage
    assert_eq!(queued.artifact_ref, "acme/widgets#42");
    assert_eq!(queued.role, "code");
    assert_eq!(queued.title, "acme/widgets#42"); // defaults to ref until forge titles land
    assert_eq!(queued.entered_at, 1000);

    let in_flight = &state.cards[&card_id_for_ref("acme/widgets#39")];
    assert_eq!(in_flight.lane, Lane::Implement); // in-flight -> implement
}

#[test]
fn role_saturation_becomes_attention_problem_not_a_lane() {
    let raw: RawDaemonSnapshot = serde_json::from_str(RAW_SNAPSHOT).expect("parses");
    let state = project_snapshot(&raw, &LaneMap::empty(), 1000);
    assert_eq!(state.problems.len(), 1);
    let problem = state.problems.values().next().expect("a problem");
    assert_eq!(problem.sev, Sev::Warn);
    assert!(problem.msg.contains("code saturated"));
    assert!(problem.msg.contains("acme/widgets#42 waiting"));
    assert_eq!(problem.card, card_id_for_ref("acme/widgets#42"));
}

#[test]
fn job_without_ref_is_skipped() {
    let raw = RawDaemonSnapshot {
        workers: RawWorkers {
            healthy: 1,
            total: 1,
        },
        queued: vec![RawJob {
            job_id: "x".to_string(),
            role: "code".to_string(),
            repo: "acme/widgets".to_string(),
            artifact_ref: None,
        }],
        in_flight: Vec::new(),
        role_saturation: Vec::new(),
    };
    let state = project_snapshot(&raw, &LaneMap::empty(), 0);
    assert!(state.cards.is_empty());
}

#[test]
fn card_id_is_dom_safe() {
    assert_eq!(card_id_for_ref("acme/widgets#42"), "acme_widgets-42");
    assert_eq!(card_id_for_ref("acme/widgets PR#8"), "acme_widgets_PR-8");
}

#[test]
fn empty_snapshot_projects_to_empty_board() {
    let raw: RawDaemonSnapshot = serde_json::from_str("{}").expect("parses defaults");
    let state = project_snapshot(&raw, &LaneMap::empty(), 0);
    assert!(state.cards.is_empty());
    assert!(state.problems.is_empty());
    assert_eq!(state.workers, Workers::default());
}
