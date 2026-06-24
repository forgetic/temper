//! Temper-specific provisioning layered on the shared throwaway Forgejo fixture.
//!
//! The process lifecycle, pinned-binary cache, and host-mode runner live in
//! `bench_forgejo`. This module re-exports those types under the
//! historical `temper_testing::forgejo_server` path and adds the Temper workflow
//! provisioning, seed, and PR-prep helpers used by ignored e2e tests.

mod admin_cache;
pub mod pr_prep;
pub mod provision;
mod provision_cache;
mod provision_rest;
pub mod provision_seed;

pub use admin_cache::{BareAdmin, CachedBareAdminServer, start_cached_bare_admin_server};
pub use bench_forgejo::{
    CachedForgejo, ForgejoRunner, ForgejoServer, ForgejoState, RunnerError, ServerError, download,
};
pub use pr_prep::{
    commit_ci_sentinel, commit_conflict_resolution_update, prepare_pull_request_head,
};
pub use provision::{
    ProvisionError, Provisioned, ProvisionedRoles, RoleIdentity, provision, provision_repository,
    provision_role_identities, provision_world,
};
pub use provision_cache::{
    CachedProvisionedServer, CachedProvisionedWorld, ProvisionedRepositories,
    start_cached_provisioned_repositories, start_cached_provisioned_server,
};
pub use provision_seed::{intake_labels, seed_intake_issue};
