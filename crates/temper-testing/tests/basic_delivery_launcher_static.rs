//! Crate-owned basic-delivery fixture contract tests.
//!
//! These intentionally exercise the bundled workflow fixture and runner config
//! APIs rather than operator-facing launchers. Editing demo scripts should not
//! affect the default Rust test suite.

use std::collections::BTreeSet;
use temper_workflow::{IntakeAuthor, RawWorkflowSpec, RoleId, ValidatedWorkflow};

fn role_ids(workflow: &ValidatedWorkflow) -> BTreeSet<&str> {
    workflow.roles().iter().map(|role| role.id.as_str()).collect()
}

fn queue_served_roles(workflow: &ValidatedWorkflow) -> BTreeSet<&str> {
    workflow
        .roles()
        .iter()
        .filter(|role| !role.queues.is_empty())
        .map(|role| role.id.as_str())
        .collect()
}

#[test]
fn bundled_basic_delivery_fixture_is_the_minimal_agent_shape() {
    let workflow = temper_testing::basic_delivery_workflow();

    assert_eq!(workflow.name(), "basic-delivery");
    assert_eq!(
        role_ids(&workflow),
        BTreeSet::from(["architect", "engineer", "mechanical"])
    );
    assert_eq!(
        queue_served_roles(&workflow),
        BTreeSet::from(["architect", "engineer"]),
        "mechanical is an automation authority, not a role worker"
    );
    assert!(matches!(
        workflow.intake_author(),
        Some(IntakeAuthor::SiteAdmin)
    ));
}

#[test]
fn bundled_basic_delivery_json_parses_to_the_public_workflow() {
    let spec: RawWorkflowSpec = serde_json::from_str(
        temper_reference_delivery::basic_delivery_workflow_json(),
    )
    .expect("bundled basic-delivery JSON parses");
    let parsed = spec
        .validate()
        .expect("bundled basic-delivery JSON validates");

    assert_eq!(parsed, temper_testing::basic_delivery_workflow());
}

#[test]
fn basic_delivery_runner_binds_only_queue_subscribing_roles() {
    let config = temper_testing::basic_delivery_runner_config();
    let bound_roles: BTreeSet<_> = config
        .role_bindings
        .iter()
        .map(|binding| binding.role.as_str())
        .collect();

    assert_eq!(bound_roles, BTreeSet::from(["architect", "engineer"]));
    for role in ["architect", "engineer"] {
        let binding = config
            .role_binding(&RoleId::new(role))
            .expect("queue-subscribing role is bound");
        assert_eq!(binding.user.id.as_str(), role);
        assert_eq!(binding.user.handle, role);
    }
    assert!(config.role_binding(&RoleId::new("mechanical")).is_none());
}
