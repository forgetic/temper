// SPDX-License-Identifier: MPL-2.0

use super::*;
use temper_workflow::{LabelId, StateId, ValidatedState, ValidatedStateDimension};

fn state(id: &str, label: Option<&str>) -> ValidatedState {
    ValidatedState {
        id: StateId::new(id),
        label: label.map(LabelId::new),
        artifacts: Vec::new(),
    }
}

fn dimension(id: &str, states: Vec<ValidatedState>) -> ValidatedStateDimension {
    ValidatedStateDimension {
        id: temper_workflow::StateDimensionId::new(id),
        exclusive: true,
        states,
    }
}

#[test]
fn named_lifecycle_states_land_in_matching_lanes() {
    let dim = dimension(
        "code_lifecycle",
        vec![
            state("untriaged", Some("untriaged")),
            state("in_progress", Some("code")),
            state("review", Some("needs-review")),
            state("ci", Some("ci-pending")),
            state("merged", Some("landed")),
        ],
    );
    let map = LaneMap::from_dimension(&dim);
    assert_eq!(map.lane_for_state(&StateId::new("untriaged")), Lane::Triage);
    assert_eq!(
        map.lane_for_state(&StateId::new("in_progress")),
        Lane::Implement
    );
    assert_eq!(map.lane_for_state(&StateId::new("review")), Lane::Review);
    assert_eq!(map.lane_for_state(&StateId::new("ci")), Lane::Ci);
    assert_eq!(map.lane_for_state(&StateId::new("merged")), Lane::Done);
}

#[test]
fn lane_resolves_by_bound_forge_label_when_id_is_opaque() {
    // State ids are opaque slugs; the bound forge label carries the human name.
    let dim = dimension(
        "lifecycle",
        vec![
            state("s0", Some("triage")),
            state("s1", Some("code")),
            state("s2", Some("landed")),
        ],
    );
    let map = LaneMap::from_dimension(&dim);
    // Looked up via the label string (as the log-tail adapter sees it).
    assert_eq!(map.lane_for_token("triage"), Lane::Triage);
    assert_eq!(map.lane_for_token("code"), Lane::Implement);
    assert_eq!(map.lane_for_token("landed"), Lane::Done);
}

#[test]
fn unrecognised_ordered_states_spread_positionally() {
    // Opaque names with no keyword hits fall back to position: first→triage,
    // last→done, the rest spread across the middle lanes.
    let dim = dimension(
        "opaque",
        vec![
            state("alpha", None),
            state("beta", None),
            state("gamma", None),
            state("delta", None),
        ],
    );
    let map = LaneMap::from_dimension(&dim);
    assert_eq!(map.lane_for_token("alpha"), Lane::Triage);
    assert_eq!(map.lane_for_token("delta"), Lane::Done);
    // middle states are somewhere between, never triage-or-done again
    let beta = map.lane_for_token("beta");
    let gamma = map.lane_for_token("gamma");
    assert!(matches!(beta, Lane::Implement | Lane::Review | Lane::Ci));
    assert!(matches!(gamma, Lane::Implement | Lane::Review | Lane::Ci));
}

#[test]
fn unknown_state_defaults_to_triage() {
    let dim = dimension("lifecycle", vec![state("code", Some("code"))]);
    let map = LaneMap::from_dimension(&dim);
    assert_eq!(map.lane_for_token("never-seen"), Lane::Triage);
}

#[test]
fn from_workflow_picks_exclusive_lifecycle_via_validate() {
    use temper_workflow::spec::{RawLabel, RawState, RawStateDimension, RawWorkflowSpec};
    let spec = RawWorkflowSpec {
        name: "demo".to_string(),
        labels: vec![
            RawLabel {
                id: "untriaged".to_string(),
                ..Default::default()
            },
            RawLabel {
                id: "code".to_string(),
                ..Default::default()
            },
            RawLabel {
                id: "landed".to_string(),
                ..Default::default()
            },
        ],
        state_dimensions: vec![RawStateDimension {
            id: "code_lifecycle".to_string(),
            exclusive: true,
            states: vec![
                RawState {
                    id: "untriaged".to_string(),
                    label: Some("untriaged".to_string()),
                    artifacts: Vec::new(),
                },
                RawState {
                    id: "in_progress".to_string(),
                    label: Some("code".to_string()),
                    artifacts: Vec::new(),
                },
                RawState {
                    id: "merged".to_string(),
                    label: Some("landed".to_string()),
                    artifacts: Vec::new(),
                },
            ],
        }],
        ..Default::default()
    };
    let validated = temper_workflow::validate(&spec).expect("spec validates");
    let map = LaneMap::from_workflow(&validated);
    assert!(map.has_lifecycle());
    assert_eq!(map.lane_for_state(&StateId::new("untriaged")), Lane::Triage);
    assert_eq!(
        map.lane_for_state(&StateId::new("in_progress")),
        Lane::Implement
    );
    assert_eq!(map.lane_for_state(&StateId::new("merged")), Lane::Done);
}

#[test]
fn workflow_without_lifecycle_falls_back_and_logs() {
    use temper_workflow::spec::RawWorkflowSpec;
    let spec = RawWorkflowSpec {
        name: "no-lifecycle".to_string(),
        ..Default::default()
    };
    let validated = temper_workflow::validate(&spec).expect("spec validates");
    let map = LaneMap::from_workflow(&validated);
    assert!(!map.has_lifecycle());
    // queued seeds triage, in-flight seeds implement
    assert_eq!(map.default_lane(false), Lane::Triage);
    assert_eq!(map.default_lane(true), Lane::Implement);
}

#[test]
fn non_exclusive_dimension_is_ignored() {
    let lifecycle = ValidatedStateDimension {
        id: temper_workflow::StateDimensionId::new("lifecycle"),
        exclusive: true,
        states: vec![state("code", Some("code")), state("merged", Some("landed"))],
    };
    let tags = ValidatedStateDimension {
        id: temper_workflow::StateDimensionId::new("tags"),
        exclusive: false,
        states: vec![
            state("urgent", Some("urgent")),
            state("blocked", Some("blocked")),
        ],
    };
    let dimensions = [tags, lifecycle];
    let chosen = choose_lifecycle_dimension(&dimensions).expect("an exclusive dimension");
    assert_eq!(chosen.id.as_str(), "lifecycle");
}
