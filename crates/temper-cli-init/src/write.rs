// SPDX-License-Identifier: MPL-2.0

//! Steps 2/3/4 of `temper init`: build the on-disk artifacts (pure), preflight
//! every target up front, write them, then write local or provisioned
//! credentials.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use temper_cli_common::{FileTargets, resolve_targets, restrict_600, write_new_file};
use temper_config::{
    ConfigInputs, CredentialInputs, ProviderKeyInput, ProviderSecretInput, ProvisionedForgeUser,
    build_config, build_credentials, forge_users_from_provisioned, write_config,
};
use temper_reference_delivery::{
    basic_delivery_workflow_json, load_workflow_document, reference_delivery_workflow_json,
};
use temper_workflow::{RawWorkflowSpec, ValidatedWorkflow};

use crate::collect::{PROVIDER_NONE, WORKFLOW_BASIC_DELIVERY, WORKFLOW_REFERENCE_DELIVERY};

use crate::collect::Answers;
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
    /// Where `workflow.yaml` is written.
    pub workflow_path: PathBuf,
    /// The workflow YAML bytes to write into the deployment bundle.
    pub workflow_yaml: String,
    /// Where the generated webhook secret is written (chmod 600).
    pub webhook_secret_path: PathBuf,
    /// The freshly generated webhook secret value.
    pub webhook_secret: String,
}

/// Builds (pure, no I/O) every artifact `temper init` will write: the config
/// document, the workflow YAML, and a freshly generated webhook secret.
///
/// `roles` come from the selected workflow's queue-subscribing roles. The
/// repo/provider come from defaults or local-dev flag overrides.
pub fn build_artifacts(answers: &Answers, opts: &InitOptions) -> Result<InitArtifacts, InitError> {
    let targets: FileTargets =
        resolve_targets(&opts.options, &opts.env, &opts.paths).map_err(InitError::Path)?;

    // Place workflow.yaml + webhook-secret beside config.toml.
    let config_dir = targets
        .config
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let workflow_path = config_dir.join("workflow.yaml");
    let webhook_secret_path = config_dir.join(WEBHOOK_SECRET_FILE);

    let workflow = workflow_artifact(&answers.workflow)?;
    let roles = workflow_roles(&workflow.validated);
    let webhook_secret = generate_secret();

    let mut config = build_config(&ConfigInputs {
        forge_url: Some(answers.forge_url.clone()),
        forge_kind: Some("forgejo".to_string()),
        repos: answers.repo_paths(),
        roles,
        workflow_path: Some(workflow_path.display().to_string()),
        webhook_addr: Some(bind_addr(&answers.webhook_addr)),
        admin_user: Some(answers.admin_user.clone()),
        ci_user: Some(temper_provision::BOT_USER.to_string()),
        provider: active_provider_name(answers),
        provider_url: answers.provider_url.clone(),
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
        workflow_yaml: workflow.yaml,
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

/// Writes `config.toml`, `workflow.yaml`, and the webhook secret (chmod 600).
/// Credentials are written separately, after provisioning.
pub fn write_artifacts(artifacts: &InitArtifacts, force: bool) -> Result<(), InitError> {
    write_config(&artifacts.config, &artifacts.config_path, force)
        .map_err(|error| InitError::Write(error.to_string()))?;
    write_new_file(&artifacts.workflow_path, &artifacts.workflow_yaml, force)
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

/// Builds + writes the local `credentials.toml` (chmod 600) before any forge
/// mutation. It contains only secrets the operator supplied locally: the admin
/// password (so a later apply can mint a token) and any provider key. Provisioned
/// role/bot tokens are added only by [`write_provisioned_credentials`].
pub fn write_local_credentials(
    answers: &Answers,
    artifacts: &InitArtifacts,
    force: bool,
) -> Result<(), InitError> {
    let mut forge_users = BTreeMap::new();
    forge_users.insert(
        answers.admin_user.clone(),
        temper_config::forge_user(None, None, Some(answers.admin_password.clone()), None),
    );
    write_credentials_with_users(answers, artifacts, forge_users, force)
}

/// Builds + writes `credentials.toml` (chmod 600) from the admin identity (Q4),
/// the minted role/bot identities, and any provider key.
pub fn write_provisioned_credentials(
    answers: &Answers,
    artifacts: &InitArtifacts,
    outcome: &ProvisionOutcome,
    force: bool,
) -> Result<(), InitError> {
    let forge_users = provisioned_forge_users(answers, outcome);
    write_credentials_with_users(answers, artifacts, forge_users, force)
}

/// Maps a provisioning outcome into the `[forge.users.*]` credentials table.
pub fn provisioned_forge_users(
    answers: &Answers,
    outcome: &ProvisionOutcome,
) -> BTreeMap<String, temper_config::ForgeUser> {
    let mut forge_users = provisioned_role_and_bot_users(outcome);
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
    forge_users
}

/// Maps the provisioned role identities plus the automation bot into
/// `[forge.users.*]` entries, leaving the admin/default identity to the caller.
pub fn provisioned_role_and_bot_users(
    outcome: &ProvisionOutcome,
) -> BTreeMap<String, temper_config::ForgeUser> {
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

    forge_users_from_provisioned(&provisioned_users)
}

fn write_credentials_with_users(
    answers: &Answers,
    artifacts: &InitArtifacts,
    forge_users: BTreeMap<String, temper_config::ForgeUser>,
    force: bool,
) -> Result<(), InitError> {
    let credentials = if let Some(provider_key) = &answers.provider_key {
        build_credentials(&CredentialInputs {
            forge_users,
            provider_key: ProviderKeyInput {
                provider: answers.provider.clone(),
                secret: ProviderSecretInput::ApiKey(provider_key.clone()),
            },
        })
    } else {
        temper_config::Credentials {
            schema_version: temper_config::SCHEMA_VERSION,
            forge: temper_config::ForgeCredentials { users: forge_users },
            ..Default::default()
        }
    };

    temper_config::write_credentials(&credentials, &artifacts.credentials_path, force)
        .map_err(|error| InitError::Write(error.to_string()))
}

fn active_provider_name(answers: &Answers) -> Option<String> {
    if answers.provider == PROVIDER_NONE {
        None
    } else {
        Some(answers.provider.clone())
    }
}

struct WorkflowArtifact {
    yaml: String,
    validated: ValidatedWorkflow,
}

fn workflow_artifact(selection: &str) -> Result<WorkflowArtifact, InitError> {
    match selection {
        WORKFLOW_BASIC_DELIVERY => {
            builtin_workflow_artifact(WORKFLOW_BASIC_DELIVERY, basic_delivery_workflow_json())
        }
        WORKFLOW_REFERENCE_DELIVERY => builtin_workflow_artifact(
            WORKFLOW_REFERENCE_DELIVERY,
            reference_delivery_workflow_json(),
        ),
        path => load_workflow_artifact(path),
    }
}

fn builtin_workflow_artifact(name: &str, json: &str) -> Result<WorkflowArtifact, InitError> {
    let spec: RawWorkflowSpec = serde_json::from_str(json).map_err(|error| {
        InitError::Unsupported(format!(
            "built-in workflow `{name}` could not be parsed as JSON: {error}"
        ))
    })?;
    let validated = spec.validate().map_err(|errors| {
        InitError::Unsupported(format!(
            "built-in workflow `{name}` failed validation:\n{errors}"
        ))
    })?;
    let yaml = serde_yaml::to_string(&spec).map_err(|error| {
        InitError::Unsupported(format!(
            "built-in workflow `{name}` could not be rendered as YAML: {error}"
        ))
    })?;
    Ok(WorkflowArtifact { yaml, validated })
}

fn load_workflow_artifact(path: &str) -> Result<WorkflowArtifact, InitError> {
    let path = Path::new(path);
    let document =
        load_workflow_document(path).map_err(|error| InitError::Unsupported(error.to_string()))?;
    let yaml = serde_yaml::to_string(&document.spec).map_err(|error| {
        InitError::Unsupported(format!(
            "workflow file {} could not be rendered as YAML: {error}",
            path.display()
        ))
    })?;
    Ok(WorkflowArtifact {
        yaml,
        validated: document.workflow,
    })
}

/// The roles `temper init` drives, derived from the selected workflow's
/// queue-subscribing roles (the same set the runner binds during provisioning).
fn workflow_roles(workflow: &ValidatedWorkflow) -> Vec<String> {
    workflow
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
