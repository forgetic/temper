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
//!   (forge URL, engine/worker knobs, which agent provider + models to use);
//! - the **credentials** file ([`Credentials`]) holds secrets (per-user forge
//!   passwords/tokens and LLM provider OAuth/api-key material).

use std::collections::BTreeMap;

use serde::Deserialize;

/// The schema version this binary reads and writes. A file declaring any other
/// version is rejected with a clear message (see [`crate::load`]).
pub const SCHEMA_VERSION: u32 = 1;

// ── config file ────────────────────────────────────────────────────────────

/// The temper **config** file: non-secret deployment settings.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Schema version (validated against [`SCHEMA_VERSION`] before this struct
    /// is built; defaulted only so a version-less in-memory `Config` is valid).
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default)]
    pub forge: ForgeConfig,
    #[serde(default)]
    pub engine: EngineConfig,
    #[serde(default)]
    pub worker: WorkerConfig,
    #[serde(default)]
    pub agent: AgentConfig,
}

/// `[forge]` — which forge backend and how to reach it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForgeConfig {
    /// Backend kind. Only `"forgejo"` is supported today.
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    /// Base URL, e.g. `http://localhost:3000`.
    #[serde(default)]
    pub url: Option<String>,
    /// The admin/default user — the key into `[forge.users.<admin>]` whose token
    /// becomes the daemon's default forge identity and drives provisioning.
    #[serde(default)]
    pub admin: Option<String>,
    /// The user whose web-UI password authenticates CI status reads (ADR 0019).
    /// Defaults to `"bot"` when present in the credentials file.
    #[serde(default)]
    pub ci_user: Option<String>,
}

/// `[engine]` — the orchestrator: what to orchestrate and how often.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineConfig {
    /// Full `host:port` bind address. Takes precedence over [`port`](Self::port).
    #[serde(default)]
    pub bind: Option<String>,
    /// Convenience: bind `127.0.0.1:<port>` when `bind` is not given.
    #[serde(default)]
    pub port: Option<u16>,
    /// Path to the workflow definition (JSON). Defaults to the bundled
    /// reference-delivery workflow when omitted.
    #[serde(default)]
    pub workflow: Option<String>,
    /// Repositories to orchestrate, each `owner/name`.
    #[serde(default)]
    pub repos: Option<Vec<String>>,
    /// Workflow roles to drive (e.g. `architect`, `engineer`, `code-reviewer`).
    #[serde(default)]
    pub roles: Option<Vec<String>>,
    /// Poll-backstop cadence in seconds.
    #[serde(default)]
    pub poll_cadence_secs: Option<u64>,
    /// Mechanical-backstop cadence in seconds. Omit to disable the mechanical
    /// backstop (label transitions / lease-gated PR landing).
    #[serde(default)]
    pub mechanical_cadence_secs: Option<u64>,
    /// Lease TTL in seconds.
    #[serde(default)]
    pub lease_ttl_secs: Option<u64>,
    /// Stable daemon identity used for lease ownership.
    #[serde(default)]
    pub daemon_id: Option<String>,
    /// Path to the file holding the Forgejo webhook HMAC secret. Omit to run
    /// without a webhook listener (poll-only).
    #[serde(default)]
    pub webhook_secret_file: Option<String>,
}

/// `[worker]` — the orchestration worker: where it works and how it reaches the
/// engine.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerConfig {
    /// Top-level directory under which per-job agent workspaces are prepared.
    #[serde(default)]
    pub workspace: Option<String>,
    /// Stable worker identity.
    #[serde(default)]
    pub worker_id: Option<String>,
    /// Engine URL to long-poll in a distributed topology. Defaults to
    /// `http://127.0.0.1:<engine.port>` (only meaningful for `--service worker`).
    #[serde(default)]
    pub daemon_url: Option<String>,
    /// Git base URL the agent pushes branches to. Defaults to `forge.url`.
    #[serde(default)]
    pub git_base_url: Option<String>,
    /// How many jobs one worker runs at once (design point: 1).
    #[serde(default)]
    pub max_concurrent_jobs: Option<u32>,
    /// Long-poll wait in milliseconds.
    #[serde(default)]
    pub poll_wait_ms: Option<u64>,
    /// Heartbeat interval in milliseconds.
    #[serde(default)]
    pub heartbeat_interval_ms: Option<u64>,
    /// Explicit `owner/name:role` capabilities. Defaults to the cross-product of
    /// `engine.repos` and `engine.roles` (one worker covers the whole feed).
    #[serde(default)]
    pub capabilities: Option<Vec<String>>,
}

/// `[agent]` — the coding agent: which LLM provider, models, and limits.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConfig {
    /// Which provider profile under `[agent.providers.*]` to use: `anthropic`,
    /// `deepseek`, or `chatgpt`.
    #[serde(default)]
    pub provider: Option<String>,
    /// Maximum model iterations per job.
    #[serde(default)]
    pub max_iterations: Option<usize>,
    /// Enable the in-workspace `investigate` sub-agent tool.
    #[serde(default)]
    pub enable_subagents: Option<bool>,
    /// Optional agent config directory (prompt overlays).
    #[serde(default)]
    pub config_dir: Option<String>,
    /// Provider profiles, keyed by provider name.
    #[serde(default)]
    pub providers: BTreeMap<String, AgentProviderConfig>,
}

/// `[agent.providers.<name>]` — non-secret provider wiring.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentProviderConfig {
    /// Optional base-URL override (e.g. a self-hosted gateway or a test LLM).
    #[serde(default)]
    pub url: Option<String>,
    /// Model selection: `models = { main = "...", investigate = "..." }`.
    #[serde(default)]
    pub models: Option<ModelMap>,
}

/// The `models` inline table on a provider profile.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelMap {
    /// The main coding model.
    #[serde(default)]
    pub main: Option<String>,
    /// The (cheaper) model for read-only `investigate` sub-agents.
    #[serde(default)]
    pub investigate: Option<String>,
}

// ── credentials file ─────────────────────────────────────────────────────────

/// The temper **credentials** file: secrets, kept out of the config file.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credentials {
    /// Schema version (validated against [`SCHEMA_VERSION`] before this struct
    /// is built).
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default)]
    pub forge: ForgeCredentials,
    #[serde(default)]
    pub agent: AgentCredentials,
}

/// `[forge]` of the credentials file.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForgeCredentials {
    /// Per-user forge credentials, keyed by user name. The key doubles as the
    /// role name for per-role identities, and is referenced by `forge.admin` /
    /// `forge.ci_user` in the config file.
    #[serde(default)]
    pub users: BTreeMap<String, ForgeUser>,
}

/// `[forge.users.<name>]` — one forge identity.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForgeUser {
    /// Forge login/username. Defaults to the section key.
    #[serde(default)]
    pub user: Option<String>,
    /// Git commit email. Defaults to `<user>@noreply.localhost`.
    #[serde(default)]
    pub email: Option<String>,
    /// Web-UI password (used for CI reads and provisioning).
    #[serde(default)]
    pub password: Option<String>,
    /// REST API token.
    #[serde(default)]
    pub token: Option<String>,
}

/// `[agent]` of the credentials file.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentCredentials {
    /// Per-provider secret material, keyed by provider name (matching
    /// `[agent.providers.<name>]` in the config file).
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderCredential>,
}

/// `[agent.providers.<name>]` of the credentials file — the provider secret.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCredential {
    /// `"oauth"` (access/refresh/expires) or `"api-key"` (key).
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    /// OAuth access token.
    #[serde(default)]
    pub access: Option<String>,
    /// OAuth refresh token.
    #[serde(default)]
    pub refresh: Option<String>,
    /// OAuth access-token expiry, unix milliseconds.
    #[serde(default)]
    pub expires: Option<i64>,
    /// API key (for `type = "api-key"` providers such as DeepSeek).
    #[serde(default)]
    pub key: Option<String>,
    /// Path to an existing pi-format `auth.json` to use as-is, instead of inline
    /// OAuth fields.
    #[serde(default)]
    pub auth_file: Option<String>,
}
