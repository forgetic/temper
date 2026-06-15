//! Top-level orchestration tying world provisioning, webhook, and seeding.
//!
//! This is the demo Forgejo operator entry point: it builds a forge handle (via
//! `temper_forge::factory::new_forgejo_provisioning`) authenticated with the
//! admin token, distills a [`ProvisionPlan`](temper_provision::ProvisionPlan)
//! (with the demo CI seed commits and, when requested, a webhook), runs the
//! backend-agnostic [`temper_provision::provision`] orchestration, then seeds
//! the entry intake issue with the workflow-resolved **author** token (which may
//! differ from the admin token).

use std::path::Path;

use std::sync::Arc;

use temper_forge::config::{FORGEJO_ROLE_PASSWORD as ROLE_PASSWORD, ForgejoConfig};
use temper_forge::{
    ItemNumber, ProvisioningForge, RepositoryPath, TokenScope, WebhookEvents, WebhookSpec,
};
use temper_provision::{ProvisionPlan, Provisioned, resolve_intake_seed_token, seed_intake_issue};
use temper_reference_delivery::{ci_seed_commits, repo_input, runner_config_for};
use temper_runner::RoleBinding;
use temper_workflow::ValidatedWorkflow;

use super::options::ProvisionOptions;
use super::{BOT_USER, IntakeIssueSeed, ProvisionError, Result};

/// Token scopes role workers need for the reference-delivery demo, matching the
/// set the previous `temper-forgejo-ops` helper emitted.
const ROLE_TOKEN_SCOPES: &[TokenScope] = &[
    TokenScope::WriteRepository,
    TokenScope::WriteIssue,
    TokenScope::WriteUser,
    TokenScope::ReadOrg,
];

#[allow(clippy::too_many_arguments)]
pub async fn provision_and_seed(
    _cx: &temper_engine_io::Cx,
    base_url: &str,
    admin_token: &str,
    owner: &str,
    name: &str,
    webhook_url: Option<&str>,
    webhook_secret_file: Option<&Path>,
    intake_seed: Option<&IntakeIssueSeed>,
    workflow: &ValidatedWorkflow,
    options: ProvisionOptions,
) -> Result<(Provisioned, Option<ItemNumber>)> {
    let config = runner_config_for(workflow, repo_input());
    let default_branch = config.repository.default_branch.clone();

    let mut plan = build_plan(
        workflow,
        owner,
        name,
        &default_branch,
        &config.role_bindings,
        options,
    )?;

    // Fold the optional webhook into the plan; the orchestration registers it
    // idempotently after the repo exists.
    if let Some(webhook_url) = webhook_url {
        let Some(secret_file) = webhook_secret_file else {
            return Err(ProvisionError::Shape {
                what: "webhook secret".into(),
                detail: "--webhook-url requires --webhook-secret-file".into(),
            });
        };
        let secret = std::fs::read_to_string(secret_file)?.trim().to_string();
        plan.webhook = Some(WebhookSpec {
            url: webhook_url.to_string(),
            secret,
            events: WebhookEvents::All,
        });
    }

    let forge = admin_forge(base_url, admin_token, owner, name);
    let provisioned =
        temper_provision::provision(&plan, forge.as_ref(), forge.as_ref(), forge.as_ref()).await?;

    // Seed the entry intake issue with the workflow-resolved author token (the
    // site admin, or a provisioned role's token), which may differ from the
    // admin token used for the rest of provisioning. Building a fresh forge
    // bound to that token authors the issue as that identity.
    let issue = if let Some(seed) = intake_seed {
        let seed_token = resolve_intake_seed_token(workflow, &provisioned, admin_token)?;
        let author_forge = admin_forge(base_url, seed_token, owner, name);
        Some(
            seed_intake_issue(
                author_forge.as_ref(),
                &provisioned.repository,
                seed,
                workflow,
            )
            .await?,
        )
    } else {
        None
    };
    Ok((provisioned, issue))
}

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
    let provisioned =
        temper_provision::provision(&plan, forge.as_ref(), forge.as_ref(), forge.as_ref()).await?;
    Ok(provisioned)
}

/// Builds a provisioning-capable forge authenticated with the admin token for
/// `owner/name`. Returned as `Arc<dyn ProvisioningForge>` so it upcasts to the
/// `&dyn Forge` / `&dyn ForgeContent` / `&dyn ForgeAdmin` the orchestration needs.
fn admin_forge(
    base_url: &str,
    admin_token: &str,
    owner: &str,
    name: &str,
) -> Arc<dyn ProvisioningForge> {
    let config = ForgejoConfig::new(base_url, admin_token).with_default_repo(owner, name);
    temper_forge::factory::new_forgejo_provisioning(config)
}

/// Distills a [`ProvisionPlan`] for the throwaway/existing-repo flows: role
/// bindings as input, the `bot` automation login, the Forgejo role-token scopes,
/// the shared role password, and the demo CI seed commits (skipped by the
/// orchestration when `existing_repo`).
///
/// The intake issue is **not** placed on the plan: its author identity (per the
/// workflow's `intake_author` knob) is resolved and seeded separately by
/// [`provision_and_seed`] with the author's token, so it must not be authored by
/// the admin handle here.
fn build_plan(
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
