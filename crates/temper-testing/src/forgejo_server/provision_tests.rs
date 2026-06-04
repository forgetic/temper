use super::*;

#[test]
fn workflow_is_host_mode_and_gated_on_commit_marker() {
    // The fail→pass mechanism: `runs-on: host` (Phase 1b) and a gate on the
    // `GITHUB_SHA` commit message carrying the CI-pass marker (no checkout
    // needed), which the engineer's fix commit adds. Both must be present
    // for Phase 4 to drive `ci_fails_then_passes`.
    assert!(CI_WORKFLOW.contains("runs-on: host"));
    assert!(CI_WORKFLOW.contains(CI_PASS_MARKER));
    assert!(CI_WORKFLOW.contains("GITHUB_SHA"));
    assert!(CI_WORKFLOW.contains("/git/commits/"));
    assert!(!CI_WORKFLOW.contains("github.event.head_commit.message"));
}

#[test]
fn role_identity_debug_redacts_secrets() {
    let identity = RoleIdentity {
        user: "engineer".into(),
        email: "engineer@example.invalid".into(),
        token: "super-secret-token".into(),
        password: "super-secret-password".into(),
    };
    let rendered = format!("{identity:?}");
    assert!(rendered.contains("engineer"));
    assert!(!rendered.contains("super-secret-token"));
    assert!(!rendered.contains("super-secret-password"));
    assert!(rendered.contains("<redacted>"));
}

#[test]
fn provisioned_role_lookup_uses_role_map() {
    let provisioned = shared_roles().for_repository("service", RepositoryId::new("acme/service"));
    assert!(provisioned.role(&RoleId::new("engineer")).is_some());
    assert!(provisioned.role(&RoleId::new("nobody")).is_none());
}

#[test]
fn provisioned_debug_redacts_admin_token() {
    let provisioned = shared_roles().for_repository("service", RepositoryId::new("acme/service"));
    let rendered = format!("{provisioned:?}");
    assert!(!rendered.contains("admin-token"));
    assert!(rendered.contains("<redacted>"));
}

#[test]
fn shared_role_identities_materialize_two_repos_without_reminting() {
    let roles = shared_roles();
    let first = roles.for_repository("service-a", RepositoryId::new("acme/service-a"));
    let second = roles.for_repository("service-b", RepositoryId::new("acme/service-b"));

    let role = RoleId::new("engineer");
    assert_eq!(
        first.role(&role).map(|identity| identity.token.as_str()),
        Some("engineer-token")
    );
    assert_eq!(
        second.role(&role).map(|identity| identity.token.as_str()),
        Some("engineer-token")
    );
    assert_eq!(first.roles.len(), second.roles.len());
}

fn shared_roles() -> ProvisionedRoles {
    let mut roles = BTreeMap::new();
    roles.insert(
        RoleId::new("engineer"),
        RoleIdentity {
            user: "engineer".into(),
            email: "engineer@example.invalid".into(),
            token: "engineer-token".into(),
            password: ROLE_PASSWORD.into(),
        },
    );
    ProvisionedRoles {
        admin_token: "admin-token".into(),
        owner: "acme".into(),
        roles,
    }
}
