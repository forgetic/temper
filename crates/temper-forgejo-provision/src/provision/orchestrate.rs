//! Top-level orchestration tying world provisioning, webhook, and seeding.

use std::path::Path;

use temper_forge::ItemNumber;
use temper_workflow::ValidatedWorkflow;

use temper_forgejo_ops::forgejo_rest;
use temper_reference_delivery::{repo_input, runner_config_for};

use super::intake::{resolve_intake_seed_token, seed_intake_issue};
use super::model::{IntakeIssueSeed, ProvisionError, ProvisionOptions, Provisioned, Result};
use super::world::provision_world;

#[allow(clippy::too_many_arguments)]
pub async fn provision_and_seed(
    cx: &temper_engine_io::Cx,
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
    let provisioned = provision_world(
        cx,
        base_url,
        admin_token,
        owner,
        name,
        &config.role_bindings,
        &config.repository.default_branch,
        workflow,
        options,
    )
    .await?;
    if let Some(webhook_url) = webhook_url {
        let Some(secret_file) = webhook_secret_file else {
            return Err(ProvisionError::Shape {
                what: "webhook secret".into(),
                detail: "--webhook-url requires --webhook-secret-file".into(),
            });
        };
        let secret = std::fs::read_to_string(secret_file)?.trim().to_string();
        forgejo_rest::ensure_repo_webhook(
            &forgejo_rest::http_client(cx.clone())?,
            base_url,
            admin_token,
            owner,
            name,
            webhook_url,
            &secret,
        )
        .await?;
    }
    let issue = if let Some(seed) = intake_seed {
        let seed_token = resolve_intake_seed_token(workflow, &provisioned, admin_token)?;
        Some(seed_intake_issue(base_url, seed_token, owner, name, seed, workflow).await?)
    } else {
        None
    };
    Ok((provisioned, issue))
}
