// SPDX-License-Identifier: MPL-2.0

//! Steps 2/3/4 of `temper init`: build the on-disk artifacts (pure), preflight
//! every target up front, write them, then write local or provisioned
//! credentials.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use temper_cli_common::{FileTargets, resolve_targets, restrict_600, write_new_file};
use temper_config::{
    AgentProfileConfig, ConfigInputs, CredentialInputs, NamedSecret, NamedSecretEntry,
    ProviderKeyInput, ProviderSecretInput, WorkerPoolConfig, build_config, build_credentials,
    write_config,
};
use temper_reference_delivery::{
    basic_delivery_workflow_json, load_workflow_document, reference_delivery_workflow_json,
};
use temper_workflow::{RawWorkflowSpec, ValidatedWorkflow};

use crate::collect::Answers;
use crate::collect::{PROVIDER_NONE, WORKFLOW_BASIC_DELIVERY, WORKFLOW_REFERENCE_DELIVERY};
use crate::{InitError, InitOptions, InitTopology};

/// The file name of the generated webhook secret, written beside `config.toml`.
const WEBHOOK_SECRET_FILE: &str = "webhook-secret";
/// The file name of the generated workflow artifact, written beside `config.toml`.
const WORKFLOW_FILE: &str = "workflow.yaml";
/// Default config-relative worker workspace directory for generated bundles.
const WORKSPACE_DIR: &str = "workspace";
/// Target-era named secret that supplies the engine/default Forge token after apply.
const FORGE_ENGINE_TOKEN_SECRET: &str = "forge-engine-token";
/// Target-era named secret that supplies the Forgejo webhook HMAC secret.
const WEBHOOK_SECRET: &str = "webhook-secret";
/// Prefix for target-era named secrets for generated local worker tokens.
const WORKER_TOKEN_SECRET_PREFIX: &str = "worker";
/// Target-era named secret for the selected provider credential.
const AGENT_PROVIDER_SECRET: &str = "agent-provider";

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
    /// The target-era secret name that stores [`Self::worker_token`].
    pub worker_token_name: String,
    /// The freshly generated local worker-token value, stored as a named secret.
    pub worker_token: String,
}

/// Builds (pure, no I/O) every artifact `temper init` will write: the config
/// document, the workflow YAML, and freshly generated local secrets.
///
/// `roles` come from the selected workflow's queue-subscribing roles. The
/// repo/provider come from defaults, answers files, or local-dev flag overrides.
pub fn build_artifacts(answers: &Answers, opts: &InitOptions) -> Result<InitArtifacts, InitError> {
    let targets: FileTargets =
        resolve_targets(&opts.options, &opts.env, &opts.paths).map_err(InitError::Path)?;

    // Place workflow.yaml + webhook-secret beside config.toml.
    let config_dir = targets
        .config
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let workflow_path = config_dir.join(WORKFLOW_FILE);
    let webhook_secret_path = config_dir.join(WEBHOOK_SECRET_FILE);

    let workflow = workflow_artifact(&answers.workflow)?;
    let roles = workflow_roles(&workflow.validated);
    let repos = answers.repo_paths();
    let webhook_secret = generate_secret();
    let worker_token = generate_secret();
    let worker_token_name = worker_token_secret_name(opts.topology);
    let workspace = opts
        .workspace
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| WORKSPACE_DIR.to_string());

    let mut config = build_config(&ConfigInputs {
        forge_url: Some(answers.forge_url.clone()),
        forge_kind: Some("forgejo".to_string()),
        repos: repos.clone(),
        roles: roles.clone(),
        workflow_path: Some(WORKFLOW_FILE.to_string()),
        webhook_addr: Some(bind_addr(&answers.webhook_addr)),
        admin_user: Some(answers.admin_user.clone()),
        provider: active_provider_name(answers),
        provider_url: answers.provider_url.clone(),
        workspace: Some(workspace.clone()),
    });
    apply_target_shape(
        &mut config,
        opts.topology,
        &roles,
        &repos,
        &workspace,
        &worker_token_name,
        answers,
    );

    Ok(InitArtifacts {
        config_path: targets.config,
        config,
        credentials_path: targets.credentials,
        workflow_path,
        workflow_yaml: workflow.yaml,
        webhook_secret_path,
        webhook_secret,
        worker_token_name,
        worker_token,
    })
}

fn apply_target_shape(
    config: &mut temper_config::Config,
    topology: InitTopology,
    roles: &[String],
    repos: &[String],
    workspace: &str,
    worker_token_name: &str,
    answers: &Answers,
) {
    config.deployment.topology = Some(topology.as_str().to_string());

    // The target workflow field is intentionally bundle-relative. The legacy
    // field is kept byte-for-byte equal so migration compatibility paths do not
    // trip the preferred/legacy conflict guard during resolution.
    config.workflow.file = Some(WORKFLOW_FILE.to_string());
    config.engine.workflow = Some(WORKFLOW_FILE.to_string());

    // Target path metadata is primary; the legacy worker field remains for
    // existing adapters until the migration fully cuts over.
    config.paths.workspace_dir = Some(workspace.to_string());
    config.worker.workspace = Some(workspace.to_string());

    config.engine.forge_token = Some(FORGE_ENGINE_TOKEN_SECRET.to_string());
    config.engine.webhook_secret = Some(WEBHOOK_SECRET.to_string());
    config.engine.webhook_secret_file = Some(WEBHOOK_SECRET_FILE.to_string());

    let pool_name = pool_name(topology).to_string();
    let provider_enabled = active_provider_name(answers).is_some();
    let agent_profile = provider_enabled.then(|| pool_name.clone());
    config.worker.pools = vec![WorkerPoolConfig {
        name: Some(pool_name.clone()),
        roles: Some(roles.to_vec()),
        repos: Some(repos.to_vec()),
        max_concurrent_jobs: Some(1),
        agent_profile,
        worker_token: Some(worker_token_name.to_string()),
    }];

    if provider_enabled {
        config.agent.profiles.insert(
            pool_name,
            AgentProfileConfig {
                command: Some(vec!["temper".to_string(), "agent".to_string()]),
                provider: Some(answers.provider.clone()),
                provider_url: answers.provider_url.clone(),
                credential: answers
                    .provider_key
                    .as_ref()
                    .map(|_| AGENT_PROVIDER_SECRET.to_string()),
                ..AgentProfileConfig::default()
            },
        );
    }
}

fn pool_name(topology: InitTopology) -> &'static str {
    match topology {
        InitTopology::Standalone => "local",
        InitTopology::Distributed => "default",
    }
}

fn worker_token_secret_name(topology: InitTopology) -> String {
    format!("{WORKER_TOKEN_SECRET_PREFIX}-{}-token", pool_name(topology))
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
/// password (so a later apply can mint a token), the provider key when one is
/// configured, and local-development named secrets for target-era references.
/// Provisioned role/bot tokens are added only after a successful deployment-wide apply.
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

fn write_credentials_with_users(
    answers: &Answers,
    artifacts: &InitArtifacts,
    forge_users: BTreeMap<String, temper_config::ForgeUser>,
    force: bool,
) -> Result<(), InitError> {
    let provider_key = answers.provider_key.as_ref().map(|key| ProviderKeyInput {
        provider: answers.provider.clone(),
        secret: ProviderSecretInput::ApiKey(key.clone()),
    });
    let mut credentials = build_credentials(&CredentialInputs {
        forge_users,
        provider_key,
    });
    add_local_development_named_secrets(&mut credentials, answers, artifacts);

    temper_config::write_credentials(&credentials, &artifacts.credentials_path, force)
        .map_err(|error| InitError::Write(error.to_string()))
}

fn add_local_development_named_secrets(
    credentials: &mut temper_config::Credentials,
    answers: &Answers,
    artifacts: &InitArtifacts,
) {
    credentials.secrets.insert(
        WEBHOOK_SECRET.to_string(),
        webhook_secret(artifacts.webhook_secret.clone()),
    );
    credentials.secrets.insert(
        artifacts.worker_token_name.clone(),
        worker_token_secret(artifacts.worker_token.clone()),
    );
    if let Some(key) = answers.provider_key.as_ref() {
        credentials.secrets.insert(
            AGENT_PROVIDER_SECRET.to_string(),
            provider_secret(&answers.provider, key),
        );
    }
    if let Some(token) = credentials
        .forge
        .users
        .get(&answers.admin_user)
        .and_then(|user| user.token.as_ref())
        .filter(|token| !token.trim().is_empty())
        .cloned()
    {
        add_forge_engine_token_secret(credentials, token);
    }
}

pub(crate) fn add_forge_engine_token_secret(
    credentials: &mut temper_config::Credentials,
    token: String,
) {
    credentials.secrets.insert(
        FORGE_ENGINE_TOKEN_SECRET.to_string(),
        forge_token_secret(token),
    );
}

fn webhook_secret(secret: String) -> NamedSecret {
    NamedSecret::Structured(NamedSecretEntry {
        kind: Some("webhook-secret".to_string()),
        secret: Some(secret),
        ..NamedSecretEntry::default()
    })
}

fn worker_token_secret(token: String) -> NamedSecret {
    NamedSecret::Structured(NamedSecretEntry {
        kind: Some("worker-token".to_string()),
        token: Some(token),
        ..NamedSecretEntry::default()
    })
}

fn forge_token_secret(token: String) -> NamedSecret {
    NamedSecret::Structured(NamedSecretEntry {
        kind: Some("forge-token".to_string()),
        token: Some(token),
        ..NamedSecretEntry::default()
    })
}

fn provider_secret(provider: &str, key: &str) -> NamedSecret {
    NamedSecret::Structured(NamedSecretEntry {
        kind: Some("provider-credentials".to_string()),
        provider: Some(provider.to_string()),
        auth: Some("api-key".to_string()),
        api_key: Some(key.to_string()),
        ..NamedSecretEntry::default()
    })
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
