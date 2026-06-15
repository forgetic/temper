//! Forgejo provisioning adapter over the backend-agnostic `temper-provision`
//! orchestration.
//!
//! The host-neutral provisioning surface (the orchestration, the
//! `Provisioned`/`RoleIdentity`/`IntakeIssueSeed`/`ProvisionError` types, and the
//! intake-seed helpers) lives in `temper-provision`. This module owns only the
//! Forgejo-specific bits — the CI workflow YAML and sentinel, the admin-token
//! `ForgejoForge` construction, the demo intake defaults — and the `secrets.env`
//! writer.

mod model;
mod orchestrate;
mod secrets;
mod world;

// Forgejo-specific constants and the adapter's CLI-facing options.
pub use model::{CI_WORKFLOW, DEFAULT_INTAKE_BODY, DEFAULT_INTAKE_TITLE, ProvisionOptions};
pub use orchestrate::provision_and_seed;
pub use secrets::{format_secrets_env, role_token_from_secrets_file, write_secrets_file};
pub use world::provision_world;

// Host-neutral provisioning types and intake-seed helpers, re-exported from
// `temper-provision` so existing `crate::provision::*` paths keep resolving.
pub use temper_forge::AccessScope;
pub use temper_provision::{
    BOT_USER, IntakeIssueSeed, ProvisionError, Provisioned, Result, RoleIdentity,
    has_default_issue_kind, intake_labels, resolve_intake_seed_token, seed_intake_issue,
};

#[cfg(test)]
mod tests;
