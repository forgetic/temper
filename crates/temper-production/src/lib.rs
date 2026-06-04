//! Production-owned executable wiring for Temper.
//!
//! This crate owns deployable process code. It composes the reusable runner,
//! provider-neutral process responders, and Forgejo backend without depending on
//! `temper-testing` or concrete LLM SDK crates.

pub mod coding_workspace;
pub mod forgejo_prep;
pub mod forgejo_rest;
mod interaction_api;
pub mod interaction_args;
pub mod interaction_bindings;
pub mod interaction_commands;
mod interaction_http;
pub mod interaction_repl;
pub mod interaction_serve;
pub mod interaction_service;
pub mod pr_diff_guard;
pub mod provision;
pub mod provision_args;
pub mod reference_delivery_validator;
pub mod trigger;
pub mod trigger_args;
pub mod wake;
pub mod worker;
pub mod worker_args;
mod worker_external_tools;
mod worker_role_agent;

#[cfg(test)]
mod dogfood_interaction_profile_tests;
#[cfg(test)]
mod interaction_args_tests;
#[cfg(test)]
mod interaction_repl_tests;
#[cfg(test)]
mod interaction_service_tests;
#[cfg(test)]
mod interaction_source_guard_tests;

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

/// Runner config shared by the production binaries.
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
