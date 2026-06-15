//! Unit tests for plan building and intake resolution.

use std::collections::BTreeMap;

use temper_forge_model::{AccessScope, RepositoryId, RepositoryPath};
use temper_workflow::{RawIntakeAuthor, RawRole, RawWorkflowSpec, RoleId, ValidatedWorkflow};

use crate::intake::resolve_intake_seed_token;
use crate::model::{IntakeIssueSeed, ProvisionOptions, ProvisionPlan, Provisioned, RoleIdentity};

/// Builds a minimal validated workflow carrying the given `intake_author` knob
/// and a single declared role.
fn workflow_with_intake_author(author: Option<RawIntakeAuthor>) -> ValidatedWorkflow {
    let spec = RawWorkflowSpec {
        name: "knob-test".into(),
        roles: vec![RawRole {
            id: "human".into(),
            ..Default::default()
        }],
        intake_author: author,
        ..Default::default()
    };
    spec.validate().expect("minimal spec should validate")
}

fn provisioned_with_roles(role_ids: &[&str]) -> Provisioned {
    let mut roles = BTreeMap::new();
    for id in role_ids {
        roles.insert(
            RoleId::new(*id),
            RoleIdentity {
                user: (*id).into(),
                email: format!("{id}@example.invalid"),
                token: format!("{id}-tok"),
                password: "pw".into(),
            },
        );
    }
    Provisioned {
        owner: "acme".into(),
        name: "service".into(),
        repository: RepositoryId::new("r"),
        roles,
        automation: RoleIdentity {
            user: "bot".into(),
            email: "bot@example.invalid".into(),
            token: "bot-tok".into(),
            password: "bot-pw".into(),
        },
    }
}

#[test]
fn site_admin_intake_author_resolves_to_admin_token() {
    let workflow = workflow_with_intake_author(Some(RawIntakeAuthor::SiteAdmin));
    let provisioned = provisioned_with_roles(&["architect"]);
    let token = resolve_intake_seed_token(&workflow, &provisioned, "admin-tok")
        .expect("site_admin author resolves to the admin token");
    assert_eq!(token, "admin-tok");
}

#[test]
fn role_intake_author_resolves_to_role_token() {
    let workflow = workflow_with_intake_author(Some(RawIntakeAuthor::Role {
        role: "human".into(),
    }));
    let provisioned = provisioned_with_roles(&["human"]);
    let token = resolve_intake_seed_token(&workflow, &provisioned, "admin-tok")
        .expect("role author resolves to that role's minted token");
    assert_eq!(token, "human-tok");
}

#[test]
fn role_intake_author_errors_when_role_not_provisioned() {
    let workflow = workflow_with_intake_author(Some(RawIntakeAuthor::Role {
        role: "human".into(),
    }));
    let provisioned = provisioned_with_roles(&["architect"]);
    let error = resolve_intake_seed_token(&workflow, &provisioned, "admin-tok")
        .expect_err("missing role token must error");
    assert!(matches!(error, crate::ProvisionError::Shape { .. }));
}

#[test]
fn absent_intake_author_falls_back_to_human_role() {
    let workflow = workflow_with_intake_author(None);
    let provisioned = provisioned_with_roles(&["human"]);
    let token = resolve_intake_seed_token(&workflow, &provisioned, "admin-tok")
        .expect("legacy human fallback resolves");
    assert_eq!(token, "human-tok");
}

#[test]
fn from_workflow_carries_options_and_resolves_labels() {
    let workflow = temper_reference_delivery::workflow();
    let options = ProvisionOptions {
        existing_repo: false,
        roles: Vec::new(),
        automation_login: "bot".into(),
        password: "pw".into(),
        token_scopes: Vec::new(),
        labels: Vec::new(),
        seed_commits: Vec::new(),
        webhook: None,
        intake: Some(IntakeIssueSeed::new("t", "b")),
    };
    let plan = ProvisionPlan::from_workflow(
        &workflow,
        RepositoryPath::new("acme", "service"),
        "main".into(),
        AccessScope::OrgOwners,
        options,
    )
    .expect("plan builds from the reference workflow");
    assert_eq!(plan.automation_login, "bot");
    assert_eq!(plan.password, "pw");
    // The reference workflow's entry kind is the default (catch-all) issue kind,
    // so the intake issue is seeded unlabeled.
    assert!(plan.intake_labels.is_empty());
    // Every declared workflow label is queued for upsert.
    let compiled = workflow.compile();
    assert_eq!(plan.labels.len(), compiled.labels().labels().len());
}
