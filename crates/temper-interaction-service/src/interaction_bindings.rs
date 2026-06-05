//! Deployment binding config for generic interaction services.
//!
//! Interaction specs describe behavior. This module loads the separate
//! deployment file that names repositories, token environment variables, local
//! responder process paths, and service bind/auth settings.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use temper_forge::RepositoryPath;
use temper_forge_forgejo::{ForgejoConfig, ForgejoForge};
use temper_interaction::{
    CompiledInteractionSpec, ConversationProfileId, InteractionError, InteractiveResponder,
    ProcessResponder, ProcessResponderConfig, RawInteractionSpec, ResponderId,
};

use crate::interaction_service::{
    InteractionProfileRuntime, InteractionService, InteractionServiceError,
};

pub const DEFAULT_INTERACTION_BIND: &str = "127.0.0.1:39210";

/// Top-level deployment binding config.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionDeploymentBindings {
    pub forge: ForgeBinding,
    pub repository: String,
    #[serde(default)]
    pub default_profile: Option<String>,
    #[serde(default)]
    pub service: ServiceBinding,
    #[serde(default)]
    pub profiles: BTreeMap<String, ProfileBinding>,
    #[serde(default)]
    pub responders: BTreeMap<String, ProcessResponderBinding>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ForgeBinding {
    pub base_url: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceBinding {
    #[serde(default)]
    pub bind: Option<String>,
    #[serde(default)]
    pub token_env: Option<String>,
    #[serde(default)]
    pub allow_non_loopback: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileBinding {
    pub human_token_env: String,
    pub agent_token_env: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessResponderBinding {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env_allowlist: Vec<String>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

#[derive(Clone, Eq, PartialEq)]
pub struct InteractionServiceOptions {
    pub bind: SocketAddr,
    pub allow_non_loopback: bool,
    pub service_token: Option<String>,
}

impl fmt::Debug for InteractionServiceOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InteractionServiceOptions")
            .field("bind", &self.bind)
            .field("allow_non_loopback", &self.allow_non_loopback)
            .field(
                "service_token",
                &self.service_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

#[derive(Debug)]
pub enum InteractionDeploymentError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Validation(String),
    Interaction(InteractionError),
    Service(InteractionServiceError),
    Config(String),
}

impl InteractionDeploymentError {
    fn config(message: impl Into<String>) -> Self {
        Self::Config(message.into())
    }
}

impl fmt::Display for InteractionDeploymentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "I/O failed: {error}"),
            Self::Json(error) => write!(formatter, "JSON failed: {error}"),
            Self::Validation(message) | Self::Config(message) => formatter.write_str(message),
            Self::Interaction(error) => write!(formatter, "interaction config failed: {error}"),
            Self::Service(error) => write!(formatter, "interaction service failed: {error}"),
        }
    }
}

impl std::error::Error for InteractionDeploymentError {}

impl From<std::io::Error> for InteractionDeploymentError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for InteractionDeploymentError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<InteractionError> for InteractionDeploymentError {
    fn from(error: InteractionError) -> Self {
        Self::Interaction(error)
    }
}

impl From<InteractionServiceError> for InteractionDeploymentError {
    fn from(error: InteractionServiceError) -> Self {
        Self::Service(error)
    }
}

pub fn load_compiled_spec(
    path: &Path,
) -> Result<CompiledInteractionSpec, InteractionDeploymentError> {
    let text = fs::read_to_string(path)?;
    let raw: RawInteractionSpec = serde_json::from_str(&text)?;
    let validated = raw
        .validate()
        .map_err(|errors| InteractionDeploymentError::Validation(errors.to_string()))?;
    Ok(validated.compile())
}

pub fn load_bindings(
    path: &Path,
) -> Result<InteractionDeploymentBindings, InteractionDeploymentError> {
    let text = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&text)?)
}

pub fn service_options<E>(
    bindings: &InteractionDeploymentBindings,
    bind_override: Option<SocketAddr>,
    allow_non_loopback_override: bool,
    env: E,
) -> Result<InteractionServiceOptions, InteractionDeploymentError>
where
    E: Fn(&str) -> Option<String>,
{
    let bind = match bind_override {
        Some(bind) => bind,
        None => parse_bind(
            bindings
                .service
                .bind
                .as_deref()
                .unwrap_or(DEFAULT_INTERACTION_BIND),
            "service.bind",
        )?,
    };
    let allow_non_loopback = bindings.service.allow_non_loopback || allow_non_loopback_override;
    if !bind.ip().is_loopback() && !allow_non_loopback {
        validate_bind(bind, allow_non_loopback, None)?;
    }
    let service_token = match bindings
        .service
        .token_env
        .as_deref()
        .filter(|key| !key.is_empty())
    {
        Some(key) => Some(require_env(&env, key)?),
        None => None,
    };
    validate_bind(bind, allow_non_loopback, service_token.as_deref())?;
    Ok(InteractionServiceOptions {
        bind,
        allow_non_loopback,
        service_token,
    })
}

pub fn build_service<E>(
    spec: &CompiledInteractionSpec,
    bindings: &InteractionDeploymentBindings,
    env: E,
) -> Result<InteractionService, InteractionDeploymentError>
where
    E: Fn(&str) -> Option<String>,
{
    let repo_path = parse_repo(&bindings.repository)?;
    let mut responders = BTreeMap::<ResponderId, Arc<dyn InteractiveResponder>>::new();
    let mut profiles = Vec::new();
    for (raw_profile_id, binding) in &bindings.profiles {
        let profile_id = ConversationProfileId::new(raw_profile_id.clone())?;
        let manifest = spec.profile(&profile_id).cloned().ok_or_else(|| {
            InteractionDeploymentError::config(format!(
                "profile `{profile_id}` is bound but not declared in the interaction spec"
            ))
        })?;
        let responder = responder_for(&manifest.responder.id, bindings, &mut responders)?;
        let human_token = require_env(&env, &binding.human_token_env)?;
        let agent_token = require_env(&env, &binding.agent_token_env)?;
        profiles.push(InteractionProfileRuntime {
            manifest,
            human_forge: Arc::new(build_forge(&bindings.forge.base_url, &human_token)),
            agent_forge: Arc::new(build_forge(&bindings.forge.base_url, &agent_token)),
            responder,
        });
    }
    let default_profile = bindings
        .default_profile
        .as_ref()
        .map(|id| ConversationProfileId::new(id.clone()))
        .transpose()?;
    InteractionService::new(
        bindings.forge.base_url.clone(),
        repo_path,
        profiles,
        default_profile,
    )
    .map_err(Into::into)
}

fn responder_for(
    responder_id: &ResponderId,
    bindings: &InteractionDeploymentBindings,
    cache: &mut BTreeMap<ResponderId, Arc<dyn InteractiveResponder>>,
) -> Result<Arc<dyn InteractiveResponder>, InteractionDeploymentError> {
    if let Some(responder) = cache.get(responder_id) {
        return Ok(Arc::clone(responder));
    }
    let binding = bindings
        .responders
        .get(responder_id.as_str())
        .ok_or_else(|| {
            InteractionDeploymentError::config(format!(
                concat!(
                    "responder `{}` is required by a bound profile but has ",
                    "no deployment binding"
                ),
                responder_id
            ))
        })?;
    let responder =
        Arc::new(ProcessResponder::new(process_config(binding)?)?) as Arc<dyn InteractiveResponder>;
    cache.insert(responder_id.clone(), Arc::clone(&responder));
    Ok(responder)
}

fn process_config(
    binding: &ProcessResponderBinding,
) -> Result<ProcessResponderConfig, InteractionDeploymentError> {
    let timeout_secs = binding
        .timeout_secs
        .unwrap_or(ProcessResponderConfig::DEFAULT_TIMEOUT.as_secs());
    if timeout_secs == 0 {
        return Err(InteractionDeploymentError::config(
            "responder timeout_secs must be positive",
        ));
    }
    let mut config = ProcessResponderConfig::new(binding.command.clone())
        .with_args(binding.args.clone())
        .with_env_allowlist(binding.env_allowlist.clone())
        .with_timeout(Duration::from_secs(timeout_secs));
    if let Some(cwd) = binding.cwd.as_deref().filter(|cwd| !cwd.trim().is_empty()) {
        config = config.with_working_dir(cwd);
    }
    Ok(config)
}

fn build_forge(base_url: &str, token: &str) -> ForgejoForge {
    ForgejoForge::new(ForgejoConfig::new(base_url.to_string(), token.to_string()))
}

fn require_env<E>(env: &E, key: &str) -> Result<String, InteractionDeploymentError>
where
    E: Fn(&str) -> Option<String>,
{
    if key.trim().is_empty() {
        return Err(InteractionDeploymentError::config(
            "token environment variable names must not be empty",
        ));
    }
    env(key)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            InteractionDeploymentError::config(format!("missing environment variable {key}"))
        })
}

fn parse_bind(raw: &str, field: &str) -> Result<SocketAddr, InteractionDeploymentError> {
    raw.parse().map_err(|_| {
        InteractionDeploymentError::config(format!("{field} must be addr:port, got `{raw}`"))
    })
}

fn validate_bind(
    bind: SocketAddr,
    allow_non_loopback: bool,
    service_token: Option<&str>,
) -> Result<(), InteractionDeploymentError> {
    if bind.ip().is_loopback() {
        return Ok(());
    }
    if !allow_non_loopback {
        return Err(InteractionDeploymentError::config(
            "non-loopback bind requires explicit allow_non_loopback or --allow-non-loopback",
        ));
    }
    if service_token.is_none() {
        return Err(InteractionDeploymentError::config(
            "non-loopback bind requires a service.token_env with a non-empty value",
        ));
    }
    Ok(())
}

fn parse_repo(repo: &str) -> Result<RepositoryPath, InteractionDeploymentError> {
    let (owner, name) = repo.split_once('/').ok_or_else(|| {
        InteractionDeploymentError::config(format!("repository must be owner/name, got `{repo}`"))
    })?;
    if owner.is_empty() || name.is_empty() || name.contains('/') {
        return Err(InteractionDeploymentError::config(format!(
            "repository must be owner/name with non-empty parts, got `{repo}`"
        )));
    }
    Ok(RepositoryPath::new(owner, name))
}
