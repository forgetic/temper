//! Forgejo provisioning used by `temper-provision-forgejo`.

mod intake;
mod model;
mod orchestrate;
mod secrets;
mod world;

pub use intake::{has_default_issue_kind, intake_labels, seed_intake_issue};
pub use model::{
    AccessScope, BOT_USER, CI_WORKFLOW, DEFAULT_INTAKE_BODY, DEFAULT_INTAKE_TITLE, IntakeIssueSeed,
    ProvisionError, ProvisionOptions, Provisioned, Result, RoleIdentity,
};
pub use orchestrate::provision_and_seed;
pub use secrets::{format_secrets_env, role_token_from_secrets_file, write_secrets_file};
pub use world::provision_world;

#[cfg(test)]
mod tests;
