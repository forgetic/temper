//! Production-owned executable wiring for Harness.
//!
//! This crate owns deployable process code. It composes the reusable runner,
//! real LLM agents, and Forgejo backend without depending on `harness-testing`.

pub mod forgejo_prep;
pub mod forgejo_rest;
pub mod product_chat;
pub mod product_chat_args;
pub mod product_chat_repl;
pub mod provision;
pub mod provision_args;
pub mod trigger;
pub mod trigger_args;
pub mod wake;
pub mod worker;
pub mod worker_args;

#[cfg(test)]
mod product_chat_tests;

use chrono::Duration;
use harness_forge::{CreateRepository, User, UserId};
use harness_runner::RunnerConfig;
use harness_workflow::{RawWorkflowSpec, RoleId};

const FIXTURE: &str = include_str!("../../harness-workflow/fixtures/reference-delivery.json");

/// Loads the bundled reference-delivery workflow used by the demo binaries.
pub fn workflow() -> harness_workflow::ValidatedWorkflow {
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

/// Reference-delivery runner config shared by both production binaries.
pub fn runner_config() -> RunnerConfig {
    RunnerConfig::new(repo_input())
        .with_role_binding(RoleId::new("architect"), actor_user("architect"))
        .with_role_binding(RoleId::new("engineer"), actor_user("engineer"))
        .with_role_binding(RoleId::new("reviewer"), actor_user("reviewer"))
        .with_role_binding(RoleId::new("owner"), actor_user("owner"))
        .with_role_binding(RoleId::new("human"), actor_user("human"))
        .with_lease_ttl(Duration::minutes(30))
        .with_poll_interval(Duration::seconds(1))
}
