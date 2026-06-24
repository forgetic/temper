//! Crate-owned reference-delivery fixture contract tests.
//!
//! These intentionally exercise the bundled workflow fixture and runner config
//! APIs rather than operator-facing launchers. Editing demo scripts should not
//! affect the default Rust test suite.

use std::collections::BTreeSet;
use temper_workflow::{IntakeAuthor, RawWorkflowSpec, RoleId, ValidatedWorkflow};

fn role_ids(workflow: &ValidatedWorkflow) -> BTreeSet<&str> {
    workflow
        .roles()
        .iter()
        .map(|role| role.id.as_str())
        .collect()
}

fn queue_ids(workflow: &ValidatedWorkflow) -> BTreeSet<&str> {
    workflow
        .queues()
        .iter()
        .map(|queue| queue.id.as_str())
        .collect()
}

fn transition_ids(workflow: &ValidatedWorkflow) -> BTreeSet<&str> {
    workflow
        .transitions()
        .iter()
        .map(|transition| transition.id.as_str())
        .collect()
}

#[test]
fn bundled_reference_delivery_fixture_keeps_review_gated_shape() {
    let workflow = temper_testing::workflow();

    assert_eq!(workflow.name(), "reference-delivery");
    assert_eq!(
        role_ids(&workflow),
        BTreeSet::from([
            "architect",
            "engineer",
            "human",
            "mechanical",
            "owner",
            "reviewer",
        ])
    );
    match workflow.intake_author() {
        Some(IntakeAuthor::Role(role)) => assert_eq!(role.as_str(), "human"),
        other => panic!("reference intake author should be human role: {other:?}"),
    }

    let queues = queue_ids(&workflow);
    for queue in ["code_ready", "pr_needs_review", "landing"] {
        assert!(queues.contains(queue), "missing queue {queue}");
    }

    let transitions = transition_ids(&workflow);
    for transition in ["open_pr", "request_review", "review_pr", "land_pr"] {
        assert!(
            transitions.contains(transition),
            "missing transition {transition}"
        );
    }
}

#[test]
fn bundled_reference_delivery_json_parses_to_the_public_workflow() {
    let spec: RawWorkflowSpec =
        serde_json::from_str(temper_reference_delivery::reference_delivery_workflow_json())
            .expect("bundled reference-delivery JSON parses");
    let parsed = spec
        .validate()
        .expect("bundled reference-delivery JSON validates");

    assert_eq!(parsed, temper_testing::workflow());
}

#[test]
fn reference_delivery_runner_binds_served_roles_but_not_mechanical() {
    let config = temper_testing::runner_config();
    let bound_roles: BTreeSet<_> = config
        .role_bindings
        .iter()
        .map(|binding| binding.role.as_str())
        .collect();

    assert_eq!(
        bound_roles,
        BTreeSet::from(["architect", "engineer", "human", "owner", "reviewer"])
    );
    for role in ["architect", "engineer", "human", "owner", "reviewer"] {
        let binding = config
            .role_binding(&RoleId::new(role))
            .expect("queue-subscribing role is bound");
        assert_eq!(binding.user.id.as_str(), role);
        assert_eq!(binding.user.handle, role);
    }
    assert!(config.role_binding(&RoleId::new("mechanical")).is_none());
}
