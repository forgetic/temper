//! Production-owned executable wiring for Temper.
//!
//! This crate owns deployable process code. It composes the reusable runner,
//! real LLM agents, and Forgejo backend without depending on `temper-testing`.

pub mod coding_workspace;
pub mod forgejo_prep;
pub mod forgejo_rest;
pub mod pr_diff_guard;
pub mod product_chat;
mod product_chat_api;
pub mod product_chat_args;
pub mod product_chat_commands;
mod product_chat_http;
pub mod product_chat_repl;
pub mod product_chat_service;
pub mod provision;
pub mod provision_args;
pub mod trigger;
pub mod trigger_args;
pub mod wake;
pub mod worker;
pub mod worker_args;
mod worker_external_tools;

#[cfg(test)]
mod product_chat_args_tests;
#[cfg(test)]
mod product_chat_tests;

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
/// Role bindings are derived from the workflow roles so adding a user-defined
/// role to the spec does not require adding another Rust hard-coded id. The
/// demo provisioning convention keeps Forge user id == role id.
pub fn runner_config() -> RunnerConfig {
    let workflow = workflow();
    let mut config = RunnerConfig::new(repo_input())
        .with_lease_ttl(Duration::minutes(30))
        .with_poll_interval(Duration::seconds(1));
    for role in workflow.roles() {
        config.set_role_binding(role.id.clone(), actor_user(role.id.as_str()));
    }
    config
}
