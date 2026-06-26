// SPDX-License-Identifier: MPL-2.0

//! The resolved, plain-data view of a temper deployment.
//!
//! [`Resolved`] is what the composition root (the `temper` binary) consumes: the
//! config file, credentials file, environment overrides, and built-in defaults
//! collapsed into ordinary std types, with no dependency on any runtime crate.
//! The adapters that turn these into engine/worker/agent runtime types live in
//! the binary, keeping this crate dependency-light and tier-agnostic.
//!
//! Fields that have no sensible default and are only required by *some* services
//! stay `Option`; the runner asserts what it needs via the `require_*` guards,
//! so each service fails fast with a precise message when its inputs are absent.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use secrecy::SecretString;

use crate::error::ConfigError;

/// The fully resolved deployment.
///
/// Note: `Resolved` deliberately does **not** implement `Serialize`. Its secret
/// fields are [`secrecy::SecretString`]s, and the `secrecy/serde` feature is not
/// enabled, so `SecretString` is not `SerializableSecret` — a `Resolved` cannot
/// be serialized (and thus cannot leak a token to a log/snapshot/wire) even by
/// accident. The only audited path secrets take to disk is
/// [`crate::write_credentials`], which serializes the raw-`String`
/// [`crate::Credentials`] schema, never `Resolved`.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub deployment: DeploymentSettings,
    pub paths: PathSettings,
    pub forge: ForgeSettings,
    pub engine: EngineSettings,
    pub worker: WorkerSettings,
    pub agent: AgentSettings,
}

/// Resolved target-era deployment metadata.
#[derive(Debug, Clone)]
pub struct DeploymentSettings {
    pub name: Option<String>,
    pub topology: Option<DeploymentTopology>,
}

/// The supported deployment topology declarations.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum DeploymentTopology {
    Standalone,
    Distributed,
}

impl DeploymentTopology {
    /// The config-file spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            DeploymentTopology::Standalone => "standalone",
            DeploymentTopology::Distributed => "distributed",
        }
    }
}

/// Resolved target-era path information.
#[derive(Debug, Clone)]
pub struct PathSettings {
    pub state_dir: Option<PathBuf>,
    pub workspace_dir: PathBuf,
    pub workflow_file: Option<PathBuf>,
}

/// Resolved forge connection + identities.
#[derive(Debug, Clone)]
pub struct ForgeSettings {
    /// Backend kind (only [`ForgeKind::Forgejo`] today).
    pub kind: ForgeKind,
    /// Base URL with trailing slashes stripped. `None` if unset anywhere.
    pub url: Option<String>,
    /// The admin/default REST token. `None` if the admin user has no token.
    pub admin_token: Option<SecretString>,
    /// Web-UI credentials for CI status reads (ADR 0019), if a `ci_user` with a
    /// password is configured.
    pub web_ui: Option<WebUiCreds>,
    /// Role → REST token, for the engine's per-role routing applier. Absent roles
    /// fall back to the admin token.
    pub role_tokens: BTreeMap<String, SecretString>,
    /// Role → git identity, for the worker's per-role checkouts/pushes.
    pub role_identities: BTreeMap<String, GitIdentity>,
}

/// The supported forge backends.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ForgeKind {
    Forgejo,
}

/// A git author identity plus its push token.
#[derive(Debug, Clone)]
pub struct GitIdentity {
    pub user: String,
    pub email: String,
    pub token: SecretString,
}

/// Web-UI login used only for the CI read fallback. The password is a
/// [`SecretString`], so the derived `Debug` prints it as `[REDACTED]`.
#[derive(Debug, Clone)]
pub struct WebUiCreds {
    pub username: String,
    pub password: SecretString,
}

/// Resolved engine (orchestrator) settings.
#[derive(Debug, Clone)]
pub struct EngineSettings {
    pub bind: SocketAddr,
    pub repos: Vec<RepoPath>,
    pub roles: Vec<String>,
    pub workflow_file: Option<PathBuf>,
    pub poll_cadence: Duration,
    pub mechanical_cadence: Option<Duration>,
    pub lease_ttl: Duration,
    pub webhook_secret_file: Option<PathBuf>,
    pub daemon_id: String,
}

/// An `owner/name` repository path.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct RepoPath {
    pub owner: String,
    pub name: String,
}

impl RepoPath {
    /// Parses `owner/name`, rejecting empty parts or extra slashes.
    pub fn parse(raw: &str) -> Result<Self, ConfigError> {
        let parts: Vec<&str> = raw.split('/').collect();
        if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
            return Err(ConfigError::invalid(format!(
                "repository must be `owner/name` with non-empty parts, got `{raw}`"
            )));
        }
        Ok(Self {
            owner: parts[0].to_string(),
            name: parts[1].to_string(),
        })
    }

    /// `owner/name`.
    pub fn display(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

/// Resolved worker settings.
#[derive(Debug, Clone)]
pub struct WorkerSettings {
    pub worker_id: String,
    pub daemon_url: String,
    pub workspace_root: PathBuf,
    /// Git base URL; `None` falls back to `forge.url` at use.
    pub git_base_url: Option<String>,
    pub max_concurrent_jobs: u32,
    pub poll_wait: Duration,
    pub heartbeat_interval: Duration,
    pub capabilities: Vec<Capability>,
}

/// An `owner/name:role` worker capability.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Capability {
    pub repo: String,
    pub role: String,
}

/// Resolved agent settings.
#[derive(Debug, Clone)]
pub struct AgentSettings {
    pub provider: ProviderSettings,
    pub max_iterations: usize,
    pub enable_subagents: bool,
    pub config_dir: Option<PathBuf>,
}

/// Resolved LLM provider wiring.
#[derive(Debug, Clone)]
pub struct ProviderSettings {
    pub kind: ProviderKind,
    pub main_model: Option<String>,
    pub investigate_model: Option<String>,
    pub base_url: Option<String>,
    pub credential: ProviderCredential,
}

/// Which LLM provider the agent authenticates against.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ProviderKind {
    Anthropic,
    DeepSeek,
    ChatGpt,
}

impl ProviderKind {
    /// The config-file spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::DeepSeek => "deepseek",
            ProviderKind::ChatGpt => "chatgpt",
        }
    }
}

/// Resolved provider secret. The secret-bearing variants hold [`SecretString`]s,
/// so the derived `Debug` prints them as `[REDACTED]`.
#[derive(Debug, Clone)]
pub enum ProviderCredential {
    /// Inline OAuth tokens (the binary materializes these into a pi-format
    /// `auth.json` the provider stack reads + refreshes).
    OAuthInline {
        access: SecretString,
        refresh: Option<SecretString>,
        expires: i64,
    },
    /// A path to an existing pi-format `auth.json`.
    OAuthFile(PathBuf),
    /// A static API key.
    ApiKey(SecretString),
    /// No credential in the file — fall back to the provider stack's ambient
    /// resolution (env / default auth.json).
    Ambient,
}

impl ForgeSettings {
    /// The forge base URL, or a `Missing` error naming what to set.
    pub fn require_url(&self) -> Result<&str, ConfigError> {
        self.url.as_deref().ok_or_else(|| {
            ConfigError::missing("forge URL is unset; set `[forge] url` in temper.toml")
        })
    }

    /// The admin REST token, or a `Missing` error.
    pub fn require_admin_token(&self) -> Result<&SecretString, ConfigError> {
        self.admin_token.as_ref().ok_or_else(|| {
            ConfigError::missing(
                "forge admin token is unset; set a `token` under \
                 `[forge.users.<admin>]` in credentials.toml (and name the \
                 admin via `[forge] admin`)",
            )
        })
    }
}

impl EngineSettings {
    /// The repos, or a `Missing` error if none were configured.
    pub fn require_repos(&self) -> Result<&[RepoPath], ConfigError> {
        if self.repos.is_empty() {
            return Err(ConfigError::missing(
                "no repositories configured; set `[engine] repos = [\"owner/name\", …]`",
            ));
        }
        Ok(&self.repos)
    }

    /// The roles, or a `Missing` error if none were configured.
    pub fn require_roles(&self) -> Result<&[String], ConfigError> {
        if self.roles.is_empty() {
            return Err(ConfigError::missing(
                "no roles configured; set `[engine] roles = [\"engineer\", …]`",
            ));
        }
        Ok(&self.roles)
    }
}
