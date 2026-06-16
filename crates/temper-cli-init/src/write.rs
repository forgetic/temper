// SPDX-License-Identifier: MPL-2.0

//! Steps 2/3/5 of `temper init`: build the on-disk artifacts (pure), preflight
//! every target up front, write them, then write credentials from the
//! provisioning result.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use temper_cli_common::{FileTargets, resolve_targets, restrict_600, write_new_file};
use temper_config::{
    ConfigInputs, CredentialInputs, ProviderKeyInput, ProviderSecretInput, ProvisionedForgeUser,
    build_config, build_credentials, forge_users_from_provisioned, write_config,
};
use temper_reference_delivery::{basic_delivery_workflow, basic_delivery_workflow_json};

use crate::collect::{Answers, PROVIDER_DEEPSEEK};
use crate::provisioner::ProvisionOutcome;
use crate::{InitError, InitOptions};

/// The file name of the generated webhook secret, written beside `config.toml`.
const WEBHOOK_SECRET_FILE: &str = "webhook-secret";

/// The built, ready-to-write artifacts plus the resolved target paths.
#[derive(Debug, Clone)]
pub struct InitArtifacts {
    /// Where `config.toml` is written.
    pub config_path: PathBuf,
    /// The serialized config document text (already round-trip validated by
    /// [`write_config`] at write time; this is the in-memory document's source).
    pub config: temper_config::Config,
    /// Where `credentials.toml` is written.
    pub credentials_path: PathBuf,
    /// Where `workflow.json` is written.
    pub workflow_path: PathBuf,
    /// The embedded basic-delivery workflow JSON bytes.
    pub workflow_json: &'static str,
    /// Where the generated webhook secret is written (chmod 600).
    pub webhook_secret_path: PathBuf,
    /// The freshly generated webhook secret value.
    pub webhook_secret: String,
}

/// Builds (pure, no I/O) every artifact `temper init` will write: the config
/// document, the workflow JSON, and a freshly generated webhook secret.
///
/// `repos`/`roles` come from the embedded basic-delivery workflow's
/// queue-subscribing roles + the default repo — they are not asked.
pub fn build_artifacts(answers: &Answers, opts: &InitOptions) -> Result<InitArtifacts, InitError> {
    let targets: FileTargets =
        resolve_targets(&opts.options, &opts.env, &opts.paths).map_err(InitError::Path)?;

    // Place workflow.json + webhook-secret beside config.toml.
    let config_dir = targets
        .config
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let workflow_path = config_dir.join("workflow.json");
    let webhook_secret_path = config_dir.join(WEBHOOK_SECRET_FILE);

    let roles = workflow_roles();
    let webhook_secret = generate_secret();

    let mut config = build_config(&ConfigInputs {
        forge_url: Some(answers.forge_url.clone()),
        forge_kind: Some("forgejo".to_string()),
        repos: vec![answers.repo_path()],
        roles,
        workflow_path: Some(workflow_path.display().to_string()),
        webhook_addr: Some(bind_addr(&answers.webhook_addr)),
        admin_user: Some(answers.admin_user.clone()),
        ci_user: Some(temper_provision::BOT_USER.to_string()),
        provider: Some(PROVIDER_DEEPSEEK.to_string()),
        workspace: opts
            .workspace
            .as_ref()
            .map(|path| path.display().to_string()),
    });
    // Point the engine at the generated webhook secret file. `build_config` does
    // not take this (it is `temper init`-specific wiring), so set it here.
    config.engine.webhook_secret_file = Some(webhook_secret_path.display().to_string());

    Ok(InitArtifacts {
        config_path: targets.config,
        config,
        credentials_path: targets.credentials,
        workflow_path,
        workflow_json: basic_delivery_workflow_json(),
        webhook_secret_path,
        webhook_secret,
    })
}

/// Checks that none of the target files already exist (unless `force`), erroring
/// with the *complete* list of pre-existing paths so the flow never writes file
/// I then aborts at file III.
pub fn preflight_clobber(artifacts: &InitArtifacts, force: bool) -> Result<(), InitError> {
    if force {
        return Ok(());
    }
    let mut existing = Vec::new();
    for path in [
        &artifacts.config_path,
        &artifacts.credentials_path,
        &artifacts.workflow_path,
        &artifacts.webhook_secret_path,
    ] {
        if path.exists() {
            existing.push(path.clone());
        }
    }
    if existing.is_empty() {
        Ok(())
    } else {
        Err(InitError::Clobber(existing))
    }
}

/// Writes `config.toml`, `workflow.json`, and the webhook secret (chmod 600).
/// Credentials are written separately, after provisioning.
pub fn write_artifacts(artifacts: &InitArtifacts, force: bool) -> Result<(), InitError> {
    write_config(&artifacts.config, &artifacts.config_path, force)
        .map_err(|error| InitError::Write(error.to_string()))?;
    write_new_file(&artifacts.workflow_path, artifacts.workflow_json, force)
        .map_err(InitError::Write)?;
    write_new_file(
        &artifacts.webhook_secret_path,
        &artifacts.webhook_secret,
        force,
    )
    .map_err(InitError::Write)?;
    restrict_600(&artifacts.webhook_secret_path).map_err(InitError::Write)?;
    Ok(())
}

/// Builds + writes `credentials.toml` (chmod 600) from the admin identity (Q4),
/// the minted role/bot identities, and the provider key.
pub fn write_credentials(
    answers: &Answers,
    artifacts: &InitArtifacts,
    outcome: &ProvisionOutcome,
    force: bool,
) -> Result<(), InitError> {
    let provisioned = &outcome.provisioned;
    // Map the provisioned role + bot identities into the plain-data seam type,
    // keyed by user/role name, then fold them into forge.users credentials.
    let mut provisioned_users: BTreeMap<String, ProvisionedForgeUser> = BTreeMap::new();
    for (role, identity) in &provisioned.roles {
        provisioned_users.insert(
            role.as_str().to_string(),
            ProvisionedForgeUser {
                user: Some(identity.user.clone()),
                email: Some(identity.email.clone()),
                password: Some(identity.password.clone()),
                token: Some(identity.token.clone()),
            },
        );
    }
    // The automation (bot) identity: keyed by its login so `[forge] ci_user`
    // (set to BOT_USER) resolves to it.
    let bot = &provisioned.automation;
    provisioned_users.insert(
        bot.user.clone(),
        ProvisionedForgeUser {
            user: Some(bot.user.clone()),
            email: Some(bot.email.clone()),
            password: Some(bot.password.clone()),
            token: Some(bot.token.clone()),
        },
    );

    let mut forge_users = forge_users_from_provisioned(&provisioned_users);
    // The admin identity (Q4): its token is minted from the password during
    // provisioning, but the *credentials* we write store the admin's own token
    // and password so the daemon authenticates as the admin by default. Insert
    // it under the admin user key referenced by `[forge] admin`.
    forge_users.insert(
        answers.admin_user.clone(),
        temper_config::forge_user(
            None,
            None,
            Some(answers.admin_password.clone()),
            Some(outcome.admin_token.clone()),
        ),
    );

    let credentials = build_credentials(&CredentialInputs {
        forge_users,
        provider_key: ProviderKeyInput {
            provider: PROVIDER_DEEPSEEK.to_string(),
            secret: ProviderSecretInput::ApiKey(answers.provider_key.clone()),
        },
    });

    temper_config::write_credentials(&credentials, &artifacts.credentials_path, force)
        .map_err(|error| InitError::Write(error.to_string()))
}

/// The roles `temper init` drives, derived from the embedded basic-delivery
/// workflow's queue-subscribing roles (the same set the runner binds).
fn workflow_roles() -> Vec<String> {
    basic_delivery_workflow()
        .roles()
        .iter()
        .filter(|role| !role.queues.is_empty())
        .map(|role| role.id.as_str().to_string())
        .collect()
}

/// Strips a leading scheme from the webhook address so `[engine] bind` holds a
/// bare `host:port` (the daemon binds to a socket address, not a URL).
fn bind_addr(webhook_addr: &str) -> String {
    webhook_addr
        .strip_prefix("http://")
        .or_else(|| webhook_addr.strip_prefix("https://"))
        .unwrap_or(webhook_addr)
        .trim_end_matches('/')
        .to_string()
}

/// Generates a 64-hex-char webhook HMAC secret from two v4 UUIDs.
fn generate_secret() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}
