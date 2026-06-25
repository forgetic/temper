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

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

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
    #[serde(default, skip_serializing_if = "ForgeConfig::is_empty")]
    pub forge: ForgeConfig,
    #[serde(default, skip_serializing_if = "EngineConfig::is_empty")]
    pub engine: EngineConfig,
    #[serde(default, skip_serializing_if = "WorkerConfig::is_empty")]
    pub worker: WorkerConfig,
    #[serde(default, skip_serializing_if = "AgentConfig::is_empty")]
    pub agent: AgentConfig,
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
    }
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
}

impl AgentConfig {
    /// `true` when every field is unset, so the section can be omitted entirely.
    fn is_empty(&self) -> bool {
        self.provider.is_none()
            && self.max_iterations.is_none()
            && self.enable_subagents.is_none()
            && self.config_dir.is_none()
            && self.providers.is_empty()
    }
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

/// Returns the canonical JSON Schema for the current Temper `config.toml` model.
///
/// The schema mirrors [`Config`] and its nested non-secret sections. Object
/// sections use `additionalProperties: false` wherever serde currently applies
/// `deny_unknown_fields`; provider maps keep arbitrary provider names while
/// constraining each provider profile's value shape.
pub fn config_json_schema() -> Value {
    schema_object([
        (
            "$schema",
            Value::from("https://json-schema.org/draft/2020-12/schema"),
        ),
        (
            "$id",
            Value::from("https://temper.local/schemas/config.schema.json"),
        ),
        ("title", Value::from("Temper config.toml")),
        (
            "description",
            Value::from(
                "JSON Schema for the non-secret Temper config.toml file model accepted by this build.",
            ),
        ),
        ("type", Value::from("object")),
        ("additionalProperties", Value::Bool(false)),
        (
            "required",
            Value::Array(vec![Value::from("schema_version")]),
        ),
        (
            "properties",
            property_map([
                ("schema_version", schema_version_schema()),
                ("forge", forge_config_schema()),
                ("engine", engine_config_schema()),
                ("worker", worker_config_schema()),
                ("agent", agent_config_schema()),
            ]),
        ),
    ])
}

fn schema_version_schema() -> Value {
    schema_object([
        (
            "description",
            Value::from("Config schema version accepted by this Temper build."),
        ),
        ("type", Value::from("integer")),
        ("const", Value::from(SCHEMA_VERSION)),
    ])
}

fn forge_config_schema() -> Value {
    closed_object_schema(
        "Forge backend and connection settings.",
        [
            (
                "type",
                string_schema("Forge backend kind; only forgejo is supported today."),
            ),
            (
                "url",
                string_schema("Forge base URL, for example http://localhost:3000."),
            ),
            (
                "admin",
                string_schema("Default forge user key from credentials.toml."),
            ),
            (
                "ci_user",
                string_schema(
                    "Forge user key whose web UI credentials authenticate CI status reads.",
                ),
            ),
        ],
    )
}

fn engine_config_schema() -> Value {
    closed_object_schema(
        "Orchestrator settings.",
        [
            ("bind", string_schema("Full host:port bind address.")),
            (
                "port",
                integer_schema(
                    "Convenience port for binding 127.0.0.1:<port>.",
                    Some(65535),
                ),
            ),
            (
                "workflow",
                string_schema("Path to a workflow definition JSON file."),
            ),
            (
                "repos",
                string_array_schema("Repositories to orchestrate, each owner/name."),
            ),
            ("roles", string_array_schema("Workflow roles to drive.")),
            (
                "poll_cadence_secs",
                integer_schema("Poll-backstop cadence in seconds.", None),
            ),
            (
                "mechanical_cadence_secs",
                integer_schema("Mechanical-backstop cadence in seconds; 0 disables it.", None),
            ),
            (
                "lease_ttl_secs",
                integer_schema("Lease TTL in seconds.", None),
            ),
            (
                "daemon_id",
                string_schema("Stable daemon identity used for lease ownership."),
            ),
            (
                "webhook_secret_file",
                string_schema("Path to the Forgejo webhook HMAC secret file."),
            ),
        ],
    )
}

fn worker_config_schema() -> Value {
    closed_object_schema(
        "Worker settings.",
        [
            (
                "workspace",
                string_schema("Top-level directory for per-job agent workspaces."),
            ),
            ("worker_id", string_schema("Stable worker identity.")),
            (
                "daemon_url",
                string_schema("Engine URL for distributed worker long-polling."),
            ),
            (
                "git_base_url",
                string_schema("Git base URL the agent pushes branches to."),
            ),
            (
                "max_concurrent_jobs",
                integer_schema("Maximum jobs run by one worker at once.", Some(4_294_967_295)),
            ),
            (
                "poll_wait_ms",
                integer_schema("Long-poll wait in milliseconds.", None),
            ),
            (
                "heartbeat_interval_ms",
                integer_schema("Heartbeat interval in milliseconds.", None),
            ),
            (
                "capabilities",
                string_array_schema("Explicit owner/name:role capabilities."),
            ),
        ],
    )
}

fn agent_config_schema() -> Value {
    closed_object_schema(
        "Coding agent provider, model, and limit settings.",
        [
            (
                "provider",
                string_schema("Provider profile key under agent.providers."),
            ),
            (
                "max_iterations",
                integer_schema("Maximum model iterations per job.", None),
            ),
            (
                "enable_subagents",
                bool_schema("Enable the in-workspace investigate sub-agent tool."),
            ),
            (
                "config_dir",
                string_schema("Optional agent config directory for prompt overlays."),
            ),
            (
                "providers",
                object_schema(
                    "Provider profiles keyed by provider name.",
                    [],
                    agent_provider_config_schema(),
                ),
            ),
        ],
    )
}

fn agent_provider_config_schema() -> Value {
    closed_object_schema(
        "Non-secret provider profile settings.",
        [
            ("url", string_schema("Optional provider base URL override.")),
            ("models", model_map_schema()),
        ],
    )
}

fn model_map_schema() -> Value {
    closed_object_schema(
        "Provider model selections.",
        [
            ("main", string_schema("Main coding model.")),
            (
                "investigate",
                string_schema("Model for read-only investigate sub-agents."),
            ),
        ],
    )
}

fn closed_object_schema(
    description: &'static str,
    properties: impl IntoIterator<Item = (&'static str, Value)>,
) -> Value {
    object_schema(description, properties, Value::Bool(false))
}

fn object_schema(
    description: &'static str,
    properties: impl IntoIterator<Item = (&'static str, Value)>,
    additional_properties: Value,
) -> Value {
    schema_object([
        ("description", Value::from(description)),
        ("type", Value::from("object")),
        ("additionalProperties", additional_properties),
        ("properties", property_map(properties)),
    ])
}

fn string_schema(description: &'static str) -> Value {
    primitive_schema(description, "string")
}

fn bool_schema(description: &'static str) -> Value {
    primitive_schema(description, "boolean")
}

fn primitive_schema(description: &'static str, kind: &'static str) -> Value {
    schema_object([
        ("description", Value::from(description)),
        ("type", Value::from(kind)),
    ])
}

fn integer_schema(description: &'static str, maximum: Option<u64>) -> Value {
    let mut fields = vec![
        ("description", Value::from(description)),
        ("type", Value::from("integer")),
        ("minimum", Value::from(0)),
    ];
    if let Some(maximum) = maximum {
        fields.push(("maximum", Value::from(maximum)));
    }
    schema_object(fields)
}

fn string_array_schema(description: &'static str) -> Value {
    schema_object([
        ("description", Value::from(description)),
        ("type", Value::from("array")),
        (
            "items",
            schema_object([("type", Value::from("string"))]),
        ),
    ])
}

fn property_map(properties: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    let mut object = serde_json::Map::new();
    for (name, schema) in properties {
        object.insert(name.to_string(), schema);
    }
    Value::Object(object)
}

fn schema_object(fields: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    let mut object = serde_json::Map::new();
    for (name, value) in fields {
        object.insert(name.to_string(), value);
    }
    Value::Object(object)
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
