//! Temper-specific provisioning layered on the shared throwaway Forgejo fixture.
//!
//! The process lifecycle, pinned-binary cache, and host-mode runner live in
//! `temper_forgejo_fixture`. This module re-exports those types under the
//! historical `temper_testing::forgejo_server` path and adds the Temper workflow
//! provisioning, seed, and PR-prep helpers used by ignored e2e tests.

pub mod pr_prep;
pub mod provision;
mod provision_cache;
mod provision_rest;
pub mod provision_seed;

pub use pr_prep::{commit_ci_sentinel, prepare_pull_request_head};
pub use provision::{
    provision, provision_repository, provision_role_identities, provision_world, ProvisionError,
    Provisioned, ProvisionedRoles, RoleIdentity,
};
pub use provision_cache::{
    start_cached_provisioned_repositories, start_cached_provisioned_server,
    CachedProvisionedServer, CachedProvisionedWorld, ProvisionedRepositories,
};
pub use provision_seed::{intake_labels, seed_intake_issue};
pub use temper_forgejo_fixture::{
    download, CachedForgejo, ForgejoRunner, ForgejoServer, ForgejoState, RunnerError, ServerError,
};
