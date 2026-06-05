//! Reference-delivery workflow defaults shared by deployable Temper tools.
//!
//! This crate contains only lightweight demo/reference-delivery configuration:
//! the bundled workflow fixture, default repository input, role actor mapping,
//! and runner defaults. Runtime processes compose it with narrower production
//! crates instead of depending on an aggregate production crate.

use chrono::Duration;
use temper_forge::{CreateRepository, User, UserId};
use temper_runner::RunnerConfig;
use temper_workflow::RawWorkflowSpec;

const FIXTURE: &str = include_str!("../../temper-workflow/fixtures/reference-delivery.json");

/// Loads the bundled reference-delivery workflow used by the demo binaries.
pub fn workflow() -> temper_workflow::ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(FIXTURE).expect("fixture parses");
    spec.validate().expect("reference fixture validates")
}

/// Reference-delivery repository input.
pub fn repo_input() -> CreateRepository {
    CreateRepository {
        owner: "acme".into(),
        name: "service".into(),
        default_branch: "main".into(),
        description: None,
    }
}

/// Builds a Forge user whose id and handle are identical.
pub fn actor_user(role: &str) -> User {
    User {
        id: UserId::new(role),
        handle: role.into(),
        display_name: None,
        email: None,
    }
}

/// Runner config shared by the reference-delivery binaries.
///
/// Role bindings are derived from workflow roles that subscribe to queues, so
/// adding a user-defined process role to the spec does not require another Rust
/// hard-coded id. Automation-only authorities such as `mechanical` have no role
/// worker or role-decision process. The demo provisioning convention keeps Forge
/// user id == role id.
pub fn runner_config() -> RunnerConfig {
    let workflow = workflow();
    let mut config = RunnerConfig::new(repo_input())
        .with_lease_ttl(Duration::minutes(30))
        .with_poll_interval(Duration::seconds(1));
    for role in workflow
        .roles()
        .iter()
        .filter(|role| !role.queues.is_empty())
    {
        config.set_role_binding(role.id.clone(), actor_user(role.id.as_str()));
    }
    config
}
