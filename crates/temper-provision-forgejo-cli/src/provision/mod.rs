//! Forgejo provisioning glue for the reference-delivery demo CLI.
//!
//! Builds a forge handle (via `temper_forge::factory`) authenticated
//! with the admin token, distills a
//! [`ProvisionPlan`](temper_provision::ProvisionPlan) (the demo CI seed commits
//! plus an optional webhook), runs the backend-agnostic
//! [`temper_provision::provision`] orchestration, then seeds the demo intake
//! issue with the workflow-resolved author token. Also owns the
//! `credentials.toml` writer the demo launcher reads role tokens back from.
//!
//! `temper init` does NOT go through here: it inlines `temper-provision`
//! directly. This is only the demo / ignored-e2e operator path.

mod options;
mod orchestrate;
mod secrets;

pub use options::ProvisionOptions;
pub use orchestrate::{provision_and_seed, provision_world};
pub use secrets::{credentials_document, role_token_from_secrets_file, write_credentials_file};

// Host-neutral provisioning types and intake-seed helpers, re-exported so the
// CLI modules can name them without a second `temper_provision`/`temper_forge`
// import path.
pub use temper_forge::AccessScope;
pub use temper_provision::{
    BOT_USER, IntakeIssueSeed, ProvisionError, Provisioned, Result, seed_intake_issue,
};
