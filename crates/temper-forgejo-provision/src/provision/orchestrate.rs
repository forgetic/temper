//! Top-level orchestration tying world provisioning, webhook, and seeding.
//!
//! This is the thin Forgejo adapter entry point: it builds a [`ForgejoForge`]
//! authenticated with the admin token, distills a
//! [`ProvisionPlan`](temper_provision::ProvisionPlan) (with the Forgejo CI seed
//! commits and, when requested, a webhook), runs the backend-agnostic
//! [`temper_provision::provision`] orchestration, then seeds the entry intake
//! issue with the workflow-resolved **author** token (which may differ from the
//! admin token).

use std::path::Path;

use temper_forge::{ItemNumber, WebhookEvents, WebhookSpec};
use temper_provision::{resolve_intake_seed_token, seed_intake_issue};
use temper_reference_delivery::{repo_input, runner_config_for};
use temper_workflow::ValidatedWorkflow;

use super::model::ProvisionOptions;
use super::world::{admin_forge, build_plan};
use super::{IntakeIssueSeed, ProvisionError, Provisioned, Result};

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
    let provisioned = temper_provision::provision(&plan, &forge, &forge, &forge).await?;

    // Seed the entry intake issue with the workflow-resolved author token (the
    // site admin, or a provisioned role's token), which may differ from the
    // admin token used for the rest of provisioning. Building a fresh
    // `ForgejoForge` bound to that token authors the issue as that identity.
    let issue = if let Some(seed) = intake_seed {
        let seed_token = resolve_intake_seed_token(workflow, &provisioned, admin_token)?;
        let author_forge = admin_forge(base_url, seed_token, owner, name);
        Some(seed_intake_issue(&author_forge, &provisioned.repository, seed, workflow).await?)
    } else {
        None
    };
    Ok((provisioned, issue))
}
