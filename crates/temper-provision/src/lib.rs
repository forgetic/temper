//! Backend-agnostic provisioning orchestration.
//!
//! This crate orchestrates provisioning a workflow's Forge "world" — the
//! organization, role users and their tokens, the automation (bot) identity,
//! the repository, labels, seed commits, CI enablement, an optional webhook, and
//! an optional intake issue — entirely through the
//! [`Forge`](temper_forge_model::Forge),
//! [`ForgeContent`](temper_forge_model::ForgeContent), and
//! [`ForgeAdmin`](temper_forge_model::ForgeAdmin) capability traits. It contains no
//! backend-specific logic, so it works against any backend that implements the
//! provisioning surface — the in-memory and filesystem reference backends as
//! well as Forgejo.
//!
//! A caller (e.g. `temper-cli-init` for `temper init`, or the
//! `temper-provision-forgejo-cli` demo operator) supplies the host-specific
//! bits — the CI workflow file and CI sentinel to seed, the webhook event
//! preset, the role password — by building a [`ProvisionPlan`] and calling
//! [`provision`].

mod error;
mod intake;
mod model;
mod orchestrate;
mod plan;

pub use error::{ProvisionError, Result};
pub use intake::{
    has_default_issue_kind, intake_labels, resolve_intake_seed_token, seed_intake_issue,
};
pub use model::{
    BOT_USER, IntakeIssueSeed, LabelSpec, ProvisionOptions, ProvisionPlan, Provisioned,
    RoleIdentity,
};
pub use orchestrate::{provision, provision_with};

#[cfg(test)]
mod tests;
