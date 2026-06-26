// SPDX-License-Identifier: MPL-2.0

//! The on-disk TOML schema for the temper **config** and **credentials** files.
//!
//! These structs are a faithful, lossless representation of *what the file
//! contains* — every field is optional and no semantic defaults are baked in
//! here. Defaults, environment overrides, and cross-field validation all live in
//! [`crate::resolve`], so there is exactly one place that decides what a value
//! becomes. `#[serde(deny_unknown_fields)]` turns a mistyped key into a precise
//! error instead of a silently-ignored setting.
//!
//! Two files, same `schema_version`:
//!
//! - the **config** file ([`Config`]) holds non-secret deployment settings
//!   (target-era deployment/workflow/path metadata plus legacy forge,
//!   engine/worker knobs, and which agent provider + models to use);
//! - the **credentials** file ([`Credentials`]) holds secrets (per-user forge
//!   passwords/tokens and LLM provider OAuth/api-key material).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The schema version this binary reads and writes. A file declaring any other
/// version is rejected with a clear message (see [`crate::load`]).
pub const SCHEMA_VERSION: u32 = 1;

// ── config file ────────────────────────────────────────────────────────────

/// The temper **config** file: non-secret deployment settings.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Schema version (validated against [`SCHEMA_VERSION`] before this struct
    /// is built; defaulted only so a version-less in-memory `Config` is valid).
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "DeploymentConfig::is_empty")]
    pub deployment: DeploymentConfig,
    #[serde(default, skip_serializing_if = "WorkflowConfig::is_empty")]
    pub workflow: WorkflowConfig,
    #[serde(default, skip_serializing_if = "PathsConfig::is_empty")]
    pub paths: PathsConfig,
    #[serde(default, skip_serializing_if = "ForgeConfig::is_empty")]
    pub forge: ForgeConfig,
    #[serde(default, skip_serializing_if = "EngineConfig::is_empty")]
    pub engine: EngineConfig,
    #[serde(default, skip_serializing_if = "WorkerConfig::is_empty")]
    pub worker: WorkerConfig,
    #[serde(default, skip_serializing_if = "AgentConfig::is_empty")]
    pub agent: AgentConfig,
}

/// `[deployment]` — target-era deployment metadata.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentConfig {
    /// Optional deployment name. Metadata only during the migration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Topology declaration: `standalone` or `distributed`. Validated during
    /// resolution; it does not select runtime behavior yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topology: Option<String>,
}

impl DeploymentConfig {
    /// `true` when every field is unset, so the section can be omitted entirely.
    fn is_empty(&self) -> bool {
        self.name.is_none() && self.topology.is_none()
    }
}

/// `[workflow]` — target-era workflow settings.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowConfig {
    /// Path to the workflow definition. Preferred over legacy
    /// `[engine] workflow` when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

impl WorkflowConfig {
    /// `true` when every field is unset, so the section can be omitted entirely.
    fn is_empty(&self) -> bool {
        self.file.is_none()
    }
}

/// `[paths]` — target-era runtime path settings.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PathsConfig {
    /// Mutable state directory. When set, the default workspace root becomes
    /// `<state_dir>/workspace`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_dir: Option<String>,
    /// Top-level worker workspace root. Preferred over legacy
    /// `[worker] workspace` when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_dir: Option<String>,
}

impl PathsConfig {
    /// `true` when every field is unset, so the section can be omitted entirely.
    fn is_empty(&self) -> bool {
        self.state_dir.is_none() && self.workspace_dir.is_none()
    }
}

/// `[forge]` — which forge backend and how to reach it.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ForgeConfig {
    /// Backend kind. Only `"forgejo"` is supported today.
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Base URL, e.g. `http://localhost:3000`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The admin/default user — the key into `[forge.users.<admin>]` whose token
    /// becomes the daemon's default forge identity and drives provisioning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admin: Option<String>,
    /// The user whose web-UI password authenticates CI status reads (ADR 0019).
    /// Defaults to `"bot"` when present in the credentials file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ci_user: Option<String>,
}

impl ForgeConfig {
    /// `true` when every field is unset, so the section can be omitted entirely.
    fn is_empty(&self) -> bool {
        self.kind.is_none() && self.url.is_none() && self.admin.is_none() && self.ci_user.is_none()
    }
}

/// `[engine]` — the orchestrator: what to orchestrate and how often.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EngineConfig {
    /// Full `host:port` bind address. Takes precedence over [`port`](Self::port).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind: Option<String>,
    /// Convenience: bind `127.0.0.1:<port>` when `bind` is not given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Path to the workflow definition (JSON). Defaults to the bundled
    /// reference-delivery workflow when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<String>,
    /// Repositories to orchestrate, each `owner/name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repos: Option<Vec<String>>,
    /// Workflow roles to drive (e.g. `architect`, `engineer`, `code-reviewer`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roles: Option<Vec<String>>,
    /// Poll-backstop cadence in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poll_cadence_secs: Option<u64>,
    /// Mechanical-backstop cadence in seconds. The mechanical backstop (label
    /// transitions / lease-gated PR landing) runs by default; webhooks are the
    /// primary reaction path and this is the level-triggered safety net. Omit
    /// for the default cadence; set `0` to disable the mechanical worker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mechanical_cadence_secs: Option<u64>,
    /// Lease TTL in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_ttl_secs: Option<u64>,
    /// Stable daemon identity used for lease ownership.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daemon_id: Option<String>,
    /// Path to the file holding the Forgejo webhook HMAC secret. Omit to run
    /// without a webhook listener (poll-only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook_secret_file: Option<String>,
}

impl EngineConfig {
    /// `true` when every field is unset, so the section can be omitted entirely.
    fn is_empty(&self) -> bool {
        self.bind.is_none()
            && self.port.is_none()
            && self.workflow.is_none()
            && self.repos.is_none()
            && self.roles.is_none()
            && self.poll_cadence_secs.is_none()
            && self.mechanical_cadence_secs.is_none()
            && self.lease_ttl_secs.is_none()
            && self.daemon_id.is_none()
            && self.webhook_secret_file.is_none()
    }
}

/// `[worker]` — the orchestration worker: where it works and how it reaches the
/// engine.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerConfig {
    /// Top-level directory under which per-job agent workspaces are prepared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// Stable worker identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<String>,
    /// Engine URL to long-poll in a distributed topology. Defaults to
    /// `http://127.0.0.1:<engine.port>` (only meaningful for `--service worker`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daemon_url: Option<String>,
    /// Git base URL the agent pushes branches to. Defaults to `forge.url`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_base_url: Option<String>,
    /// How many jobs one worker runs at once (design point: 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent_jobs: Option<u32>,
    /// Long-poll wait in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poll_wait_ms: Option<u64>,
    /// Heartbeat interval in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heartbeat_interval_ms: Option<u64>,
    /// Explicit `owner/name:role` capabilities. Defaults to the cross-product of
    /// `engine.repos` and `engine.roles` (one worker covers the whole feed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
    /// Target-era named capability classes, declared as `[[worker.pools]]`.
    /// Parsed and validated for inspection/future dispatch, but not used by the
    /// current worker runtime yet.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pools: Vec<WorkerPoolConfig>,
}

impl WorkerConfig {
    /// `true` when every field is unset, so the section can be omitted entirely.
    fn is_empty(&self) -> bool {
        self.workspace.is_none()
            && self.worker_id.is_none()
            && self.daemon_url.is_none()
            && self.git_base_url.is_none()
            && self.max_concurrent_jobs.is_none()
            && self.poll_wait_ms.is_none()
            && self.heartbeat_interval_ms.is_none()
            && self.capabilities.is_none()
            && self.pools.is_empty()
    }
}

/// `[[worker.pools]]` — target-era named worker capability classes.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerPoolConfig {
    /// Pool name (unique after trimming). Required during resolution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Workflow roles this pool is willing to run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roles: Option<Vec<String>>,
    /// Repositories this pool is willing to run, each `owner/name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repos: Option<Vec<String>>,
    /// Maximum jobs a worker in this pool should run concurrently.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent_jobs: Option<u32>,
    /// Target-era agent profile name used by this pool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_profile: Option<String>,
    /// Secret-name reference for future worker authentication.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_token: Option<String>,
}

/// `[agent]` — the coding agent: which LLM provider, models, and limits.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConfig {
    /// Which provider profile under `[agent.providers.*]` to use: `anthropic`,
    /// `deepseek`, or `chatgpt`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Maximum model iterations per job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_iterations: Option<usize>,
    /// Enable the in-workspace `investigate` sub-agent tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_subagents: Option<bool>,
    /// Optional agent config directory (prompt overlays).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_dir: Option<String>,
    /// Provider profiles, keyed by provider name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub providers: BTreeMap<String, AgentProviderConfig>,
    /// Target-era agent profiles, keyed by profile name and declared as
    /// `[agent.profiles.<name>]`. Parsed and validated for inspection/future
    /// pool dispatch, but not selected by the active runtime yet.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<String, AgentProfileConfig>,
}

impl AgentConfig {
    /// `true` when every field is unset, so the section can be omitted entirely.
    fn is_empty(&self) -> bool {
        self.provider.is_none()
            && self.max_iterations.is_none()
            && self.enable_subagents.is_none()
            && self.config_dir.is_none()
            && self.providers.is_empty()
            && self.profiles.is_empty()
    }
}

/// `[agent.profiles.<name>]` — target-era named agent execution profile.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentProfileConfig {
    /// Agent command argv (for example `["temper", "agent"]`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    /// Provider kind: `anthropic`, `deepseek`, or `chatgpt`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Main model selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Model for read-only investigate sub-agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub investigate_model: Option<String>,
    /// Optional provider base URL override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_url: Option<String>,
    /// Maximum model iterations per job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_iterations: Option<usize>,
    /// Enable the in-workspace investigate sub-agent tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagents: Option<bool>,
    /// Secret-name reference for future provider credentials.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
}

/// `[agent.providers.<name>]` — non-secret provider wiring.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentProviderConfig {
    /// Optional base-URL override (e.g. a self-hosted gateway or a test LLM).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Model selection: `models = { main = "...", investigate = "..." }`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<ModelMap>,
}

/// The `models` inline table on a provider profile.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelMap {
    /// The main coding model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main: Option<String>,
    /// The (cheaper) model for read-only `investigate` sub-agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub investigate: Option<String>,
}

// ── credentials file ─────────────────────────────────────────────────────────

/// The temper **credentials** file: secrets, kept out of the config file.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Credentials {
    /// Schema version (validated against [`SCHEMA_VERSION`] before this struct
    /// is built).
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "ForgeCredentials::is_empty")]
    pub forge: ForgeCredentials,
    #[serde(default, skip_serializing_if = "AgentCredentials::is_empty")]
    pub agent: AgentCredentials,
}

/// `[forge]` of the credentials file.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ForgeCredentials {
    /// Per-user forge credentials, keyed by user name. The key doubles as the
    /// role name for per-role identities, and is referenced by `forge.admin` /
    /// `forge.ci_user` in the config file.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub users: BTreeMap<String, ForgeUser>,
}

impl ForgeCredentials {
    /// `true` when no users are present, so the section can be omitted entirely.
    fn is_empty(&self) -> bool {
        self.users.is_empty()
    }
}

/// `[forge.users.<name>]` — one forge identity.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ForgeUser {
    /// Forge login/username. Defaults to the section key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Git commit email. Defaults to `<user>@noreply.localhost`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Web-UI password (used for CI reads and provisioning).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// REST API token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

/// `[agent]` of the credentials file.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentCredentials {
    /// Per-provider secret material, keyed by provider name (matching
    /// `[agent.providers.<name>]` in the config file).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub providers: BTreeMap<String, ProviderCredential>,
}

impl AgentCredentials {
    /// `true` when no provider secrets are present, so the section can be
    /// omitted entirely.
    fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }
}

/// `[agent.providers.<name>]` of the credentials file — the provider secret.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCredential {
    /// `"oauth"` (access/refresh/expires) or `"api-key"` (key).
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// OAuth access token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<String>,
    /// OAuth refresh token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh: Option<String>,
    /// OAuth access-token expiry, unix milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<i64>,
    /// API key (for `type = "api-key"` providers such as DeepSeek).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Path to an existing pi-format `auth.json` to use as-is, instead of inline
    /// OAuth fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_file: Option<String>,
}
