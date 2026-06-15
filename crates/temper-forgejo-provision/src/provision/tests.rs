use std::collections::BTreeMap;

use temper_forge::RepositoryId;
use temper_forge_forgejo::ROLE_PASSWORD;
use temper_workflow::{RoleId, ValidatedWorkflow};

use super::{
    AccessScope, BOT_USER, CI_WORKFLOW, ProvisionError, ProvisionOptions, Provisioned,
    RoleIdentity, format_secrets_env, has_default_issue_kind, intake_labels,
    resolve_intake_seed_token, role_token_from_secrets_file, write_secrets_file,
};

#[test]
fn provision_options_default_is_back_compat() {
    // Defaults must reproduce today's throwaway behavior: create the repo
    // (and commit CI) and join the Owners team.
    let options = ProvisionOptions::default();
    assert!(!options.existing_repo);
    assert_eq!(options.access, AccessScope::OrgOwners);
    assert_eq!(AccessScope::default(), AccessScope::OrgOwners);
}

#[test]
fn ci_workflow_uses_commit_message_marker() {
    assert!(CI_WORKFLOW.contains("runs-on: host"));
    assert!(CI_WORKFLOW.contains(crate::forgejo_prep::CI_PASS_MARKER));
    assert!(CI_WORKFLOW.contains("GITHUB_SHA"));
    assert!(CI_WORKFLOW.contains("/git/commits/"));
    assert!(!CI_WORKFLOW.contains("github.event.head_commit.message"));
}

#[test]
fn ci_workflow_yaml_is_indented_not_flush_left() {
    // Regression for lesson 0013: a `\<newline>`-continued string literal
    // strips the YAML indentation, producing a flush-left, invalid workflow
    // that Forgejo silently fails to detect. The mapping keys MUST keep
    // their leading spaces.
    assert!(
        CI_WORKFLOW.contains("\n  build:\n"),
        "`build:` must be indented under `jobs:`"
    );
    assert!(
        CI_WORKFLOW.contains("\n    runs-on: host\n"),
        "`runs-on:` must be indented under `build:`"
    );
    assert!(
        CI_WORKFLOW.contains("\n    steps:\n"),
        "`steps:` must be indented under `build:`"
    );
    // No job-level key should appear flush-left (column 0).
    assert!(!CI_WORKFLOW.contains("\nbuild:\n"));
    assert!(!CI_WORKFLOW.contains("\nruns-on:"));
}

#[test]
fn intake_labels_resolve_to_entry_issue() {
    // The reference workflow now models raw human intake: the `intake` kind
    // is the default (catch-all) issue kind with no identifying labels, so a
    // freshly filed issue carries no labels and a mechanical queue stamps it.
    // `intake_labels` therefore resolves to no labels, and the workflow still
    // declares a default issue kind, so seeding does not error.
    let workflow = temper_reference_delivery::workflow();
    assert!(intake_labels(&workflow).is_empty());
    assert!(has_default_issue_kind(&workflow));
}

#[test]
fn role_token_round_trips_through_secrets_file() {
    let mut roles = BTreeMap::new();
    roles.insert(
        RoleId::new("code-reviewer"),
        RoleIdentity {
            user: "reviewer".into(),
            email: "reviewer@example.invalid".into(),
            token: "tok-with-'-quote".into(),
            password: ROLE_PASSWORD.into(),
        },
    );
    let env = format_secrets_env(&Provisioned {
        owner: "acme".into(),
        name: "service".into(),
        repository: RepositoryId::new("r"),
        roles,
        automation: RoleIdentity {
            user: BOT_USER.into(),
            email: "bot@example.invalid".into(),
            token: "bot-tok".into(),
            password: "bot-pw".into(),
        },
    });
    let dir = std::env::temp_dir().join(format!("temper-seed-token-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("roles.env");
    write_secrets_file(&path, &env).expect("write secrets file");

    let token = role_token_from_secrets_file(&path, &RoleId::new("code-reviewer"))
        .expect("recovers the role token");
    assert_eq!(token, "tok-with-'-quote");

    let missing = role_token_from_secrets_file(&path, &RoleId::new("architect"))
        .expect_err("absent role errors");
    assert!(matches!(missing, ProvisionError::Shape { .. }));

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn secrets_env_escapes_single_quotes() {
    let mut roles = BTreeMap::new();
    roles.insert(
        RoleId::new("code-reviewer"),
        RoleIdentity {
            user: "reviewer".into(),
            email: "reviewer@example.invalid".into(),
            token: "tok".into(),
            password: "pw-with-'-quote".into(),
        },
    );
    let env = format_secrets_env(&Provisioned {
        owner: "acme".into(),
        name: "service".into(),
        repository: RepositoryId::new("r"),
        roles,
        automation: RoleIdentity {
            user: BOT_USER.into(),
            email: "bot@example.invalid".into(),
            token: "bot-tok".into(),
            password: "bot-pw".into(),
        },
    });
    assert!(env.contains("TEMPER_FORGEJO_TOKEN_CODE_REVIEWER='tok'"));
    assert!(env.contains(r"TEMPER_FORGEJO_PASSWORD_CODE_REVIEWER='pw-with-'\''-quote'"));
    assert!(env.contains("TEMPER_FORGEJO_BOT_USER='bot'"));
    assert!(env.contains("TEMPER_FORGEJO_BOT_TOKEN='bot-tok'"));
    assert!(env.contains("TEMPER_FORGEJO_BOT_PASSWORD='bot-pw'"));
}

/// Builds a `Provisioned` whose role map contains the given role ids, each
/// with a token of `<role>-tok`.
fn provisioned_with_roles(role_ids: &[&str]) -> Provisioned {
    let mut roles = BTreeMap::new();
    for id in role_ids {
        roles.insert(
            RoleId::new(*id),
            RoleIdentity {
                user: (*id).into(),
                email: format!("{id}@example.invalid"),
                token: format!("{id}-tok"),
                password: ROLE_PASSWORD.into(),
            },
        );
    }
    Provisioned {
        owner: "acme".into(),
        name: "service".into(),
        repository: RepositoryId::new("r"),
        roles,
        automation: RoleIdentity {
            user: BOT_USER.into(),
            email: "bot@example.invalid".into(),
            token: "bot-tok".into(),
            password: "bot-pw".into(),
        },
    }
}

/// Builds a minimal validated workflow carrying the given `intake_author`
/// knob and a single declared role.
fn workflow_with_intake_author(
    author: Option<temper_workflow::RawIntakeAuthor>,
) -> ValidatedWorkflow {
    let spec = temper_workflow::RawWorkflowSpec {
        name: "knob-test".into(),
        roles: vec![temper_workflow::RawRole {
            id: "human".into(),
            ..Default::default()
        }],
        intake_author: author,
        ..Default::default()
    };
    spec.validate().expect("minimal spec should validate")
}

#[test]
fn site_admin_intake_author_resolves_to_admin_token() {
    // Acceptance: a workflow whose intake author is the site admin seeds
    // even when no `human` role was provisioned.
    let workflow = workflow_with_intake_author(Some(temper_workflow::RawIntakeAuthor::SiteAdmin));
    let provisioned = provisioned_with_roles(&["architect"]);
    let token = resolve_intake_seed_token(&workflow, &provisioned, "admin-tok")
        .expect("site_admin author resolves to the admin token");
    assert_eq!(token, "admin-tok");
}

#[test]
fn role_intake_author_resolves_to_role_token() {
    let workflow = workflow_with_intake_author(Some(temper_workflow::RawIntakeAuthor::Role {
        role: "human".into(),
    }));
    let provisioned = provisioned_with_roles(&["human"]);
    let token = resolve_intake_seed_token(&workflow, &provisioned, "admin-tok")
        .expect("role author resolves to that role's minted token");
    assert_eq!(token, "human-tok");
}

#[test]
fn role_intake_author_errors_when_role_not_provisioned() {
    let workflow = workflow_with_intake_author(Some(temper_workflow::RawIntakeAuthor::Role {
        role: "human".into(),
    }));
    let provisioned = provisioned_with_roles(&["architect"]);
    let error = resolve_intake_seed_token(&workflow, &provisioned, "admin-tok")
        .expect_err("missing role token must error");
    assert!(matches!(error, ProvisionError::Shape { .. }));
}

#[test]
fn absent_intake_author_falls_back_to_human_role() {
    let workflow = workflow_with_intake_author(None);
    let provisioned = provisioned_with_roles(&["human"]);
    let token = resolve_intake_seed_token(&workflow, &provisioned, "admin-tok")
        .expect("legacy human fallback resolves");
    assert_eq!(token, "human-tok");
}
