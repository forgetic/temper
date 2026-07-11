// SPDX-License-Identifier: MPL-2.0

//! `temper init` — generate a local deployment bundle and optionally apply it.
//!
//! Init performs local collection and writes first. With `--apply`, it then
//! reloads those files through the same deployment-wide apply path used by
//! `temper apply`, preserving one desired-state and credential-update model.

mod answers_file;
mod apply;
mod args;
mod collect;
mod deployment;
mod init_flow;
mod plan;
mod provisioner;
mod usage;
mod write;

pub use apply::{
    APPLY_USAGE, ApplyCredentialMode, ApplyOptions, apply_main, apply_main_with_options, run_apply,
};
pub use args::{InitOverrides, InitTopology, RepoSelection};
pub use collect::{Answers, collect_answers};
pub use deployment::{
    DeploymentBundle, DeploymentMetadata, DesiredRepository, DesiredWebhook, ForgeAuthentication,
    durable_credentials_path, load_deployment,
};
pub use init_flow::{InitError, InitOptions, main, main_with_options, run_init};
pub use plan::{PLAN_USAGE, PlanOptions, plan_main_with_options, run_plan};
pub use provisioner::{ApplyPlanOutcome, ApplyPlanRequest, ApplyProvisioner, ForgejoProvisioner};
pub use usage::USAGE;
pub use write::{InitArtifacts, build_artifacts, preflight_clobber, write_artifacts};
