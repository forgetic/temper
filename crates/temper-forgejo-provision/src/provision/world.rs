//! Provisioning the Forgejo "world": org, users, bot, repo, labels, and CI.
//!
//! This is now a thin Forgejo adapter over the backend-agnostic
//! [`temper_provision::provision`] orchestration: it builds a [`ForgejoForge`]
//! authenticated with the admin token, distills a
//! [`ProvisionPlan`](temper_provision::ProvisionPlan) from the workflow plus the
//! Forgejo-specific seed commits (the CI workflow + sentinel), and runs the
//! orchestration against that backend.

use temper_forge::{RepositoryPath, TokenScope};
use temper_forge_forgejo::{ForgejoConfig, ForgejoForge, ROLE_PASSWORD};
use temper_provision::{ProvisionPlan, Provisioned};
use temper_runner::RoleBinding;
use temper_workflow::ValidatedWorkflow;

use crate::forgejo_prep::ci_seed_commits;

use super::model::ProvisionOptions;
use super::{BOT_USER, Result};

/// Token scopes role workers need for the reference-delivery demo, matching the
/// set the previous `temper-forgejo-ops` helper emitted.
const ROLE_TOKEN_SCOPES: &[TokenScope] = &[
    TokenScope::WriteRepository,
    TokenScope::WriteIssue,
    TokenScope::WriteUser,
    TokenScope::ReadOrg,
];

#[allow(clippy::too_many_arguments)]
pub async fn provision_world(
    _cx: &temper_engine_io::Cx,
    base_url: &str,
    admin_token: &str,
    owner: &str,
    name: &str,
    roles: &[RoleBinding],
    default_branch: &str,
    workflow: &ValidatedWorkflow,
    options: ProvisionOptions,
) -> Result<Provisioned> {
    let forge = admin_forge(base_url, admin_token, owner, name);
    let plan = build_plan(workflow, owner, name, default_branch, roles, options)?;
    let provisioned = temper_provision::provision(&plan, &forge, &forge, &forge).await?;
    Ok(provisioned)
}

/// Builds a [`ForgejoForge`] authenticated with the admin token for `owner/name`.
pub(super) fn admin_forge(
    base_url: &str,
    admin_token: &str,
    owner: &str,
    name: &str,
) -> ForgejoForge {
    let config = ForgejoConfig::new(base_url, admin_token).with_default_repo(owner, name);
    ForgejoForge::new(config)
}

/// Distills a [`ProvisionPlan`] for the throwaway/existing-repo flows: role
/// bindings as input, the `bot` automation login, the Forgejo role-token scopes,
/// the shared role password, and the CI seed commits (skipped by the
/// orchestration when `existing_repo`).
///
/// The intake issue is **not** placed on the plan: its author identity
/// (per the workflow's `intake_author` knob) is resolved and seeded separately
/// by [`provision_and_seed`](super::orchestrate::provision_and_seed) with the
/// author's token, so it must not be authored by the admin handle here.
pub(super) fn build_plan(
    workflow: &ValidatedWorkflow,
    owner: &str,
    name: &str,
    default_branch: &str,
    roles: &[RoleBinding],
    options: ProvisionOptions,
) -> Result<ProvisionPlan> {
    let plan_options = temper_provision::ProvisionOptions {
        existing_repo: options.existing_repo,
        roles: roles.to_vec(),
        automation_login: BOT_USER.to_string(),
        password: ROLE_PASSWORD.to_string(),
        token_scopes: ROLE_TOKEN_SCOPES.to_vec(),
        labels: Vec::new(),
        seed_commits: ci_seed_commits(default_branch),
        webhook: None,
        intake: None,
    };
    let plan = ProvisionPlan::from_workflow(
        workflow,
        RepositoryPath::new(owner, name),
        default_branch.to_string(),
        options.access,
        plan_options,
    )?;
    Ok(plan)
}
