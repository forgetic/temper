// SPDX-License-Identifier: MPL-2.0

//! Programmatic **builder + writer** for the two on-disk TOML files.
//!
//! Where [`crate::resolve`] reads files into a runtime [`Resolved`](crate::Resolved),
//! this module goes the other way: it constructs the schema structs
//! ([`Config`]/[`Credentials`]) from collected answers and writes them back to
//! disk. It is the seam a future `temper init` builds on — gather inputs
//! interactively (or from a provisioning run), build the documents, write them.
//!
//! Two safety properties hold by construction:
//!
//! 1. **Round-trip validated before write.** [`write_config`]/[`write_credentials`]
//!    serialize the in-memory document to TOML, then *re-parse it through the
//!    same loader the daemon uses* ([`Config::parse`]/[`Credentials::parse`])
//!    before any byte touches disk. A document that would not load can never be
//!    written, so `temper init` cannot emit a file `temper daemon` would reject.
//! 2. **Credentials are written `0600`.** [`write_credentials`] sets owner-only
//!    permissions on Unix immediately after creating the file.
//!
//! This module adds no dependencies: it stays `serde` + `toml` only, with no
//! knowledge of the forge or the provisioner. The provisioning seam is the
//! plain-data [`ProvisionedForgeUser`] type defined *here* and the
//! [`forge_users_from_provisioned`] adapter — the caller (e.g. `temper init`,
//! issue #182) maps a `temper-provision` result into these, so this crate never
//! depends on the provisioner.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::error::{ConfigError, FileKind};
use crate::schema::{
    AgentConfig, AgentCredentials, AgentProviderConfig, Config, Credentials, DeploymentConfig,
    EngineConfig, ForgeConfig, ForgeCredentials, ForgeUser, PathsConfig, ProviderCredential,
    SCHEMA_VERSION, WorkerConfig, WorkflowConfig,
};

/// Collected, non-secret answers for [`build_config`].
///
/// Every field is optional; an unset field is simply omitted from the written
/// file, leaving the daemon's built-in default in force. Strings are taken
/// verbatim (the builder does not trim or validate — [`write_config`]'s
/// round-trip is the gate), so callers pass already-clean values.
#[derive(Debug, Clone, Default)]
pub struct ConfigInputs {
    /// `[forge] url`, e.g. `http://localhost:3000`.
    pub forge_url: Option<String>,
    /// `[forge] type` (the forge backend; only `"forgejo"` today).
    pub forge_kind: Option<String>,
    /// `[engine] repos`, each `owner/name`.
    pub repos: Vec<String>,
    /// `[engine] roles` to drive.
    pub roles: Vec<String>,
    /// `[engine] workflow` — path to a workflow JSON/YAML document, or `None`
    /// for the bundled default.
    pub workflow_path: Option<String>,
    /// `[engine] bind` — full `host:port` listen address.
    pub webhook_addr: Option<String>,
    /// `[forge] admin` — the credentials-file user key whose token is the
    /// daemon's default identity.
    pub admin_user: Option<String>,
    /// `[forge] ci_user` — the user whose web-UI password reads CI status.
    pub ci_user: Option<String>,
    /// `[agent] provider` — the active provider profile name.
    pub provider: Option<String>,
    /// `[agent.providers.<provider>].url` — base URL override for the active
    /// provider (e.g. a self-hosted gateway or a test LLM).
    pub provider_url: Option<String>,
    /// `[worker] workspace` — the top-level worker workspace root (a
    /// `~`-prefixed value is expanded at resolve time; workers create per-job
    /// scoped roots below it).
    pub workspace: Option<String>,
}

/// A provisioned forge identity, in plain data.
///
/// This is the *seam* that keeps `temper-config` free of any provision/forge
/// dependency: it mirrors the fields a provisioning run yields for one user
/// (`temper-provision`'s `RoleIdentity`) without naming that type. The
/// caller maps each provisioned identity into one of these; this crate then
/// folds them into [`ForgeUser`] credentials via [`forge_users_from_provisioned`].
#[derive(Debug, Clone, Default)]
pub struct ProvisionedForgeUser {
    /// Forge login. Omitted from the file when it equals the user key
    /// (the key is the default), keeping the output minimal.
    pub user: Option<String>,
    /// Git commit email.
    pub email: Option<String>,
    /// Web-UI password.
    pub password: Option<String>,
    /// REST API token.
    pub token: Option<String>,
}

/// The provider secret for [`CredentialInputs`], in the two shapes the schema
/// supports.
#[derive(Debug, Clone)]
pub enum ProviderSecretInput {
    /// Inline OAuth tokens (`type = "oauth"`).
    OAuth {
        access: String,
        refresh: Option<String>,
        /// Unix-millisecond expiry; `0` means "unknown / refresh on first use".
        expires: i64,
    },
    /// A static API key (`type = "api-key"`).
    ApiKey(String),
    /// A pointer to an existing pi-format `auth.json` (`auth_file = "…"`).
    AuthFile(String),
}

/// Collected answers for [`build_credentials`].
#[derive(Debug, Clone)]
pub struct CredentialInputs {
    /// Per-user forge credentials, keyed by user name (the key doubles as the
    /// role name and is referenced by `forge.admin` / `forge.ci_user`).
    pub forge_users: BTreeMap<String, ForgeUser>,
    /// The active provider's name and secret (matching `[agent] provider`).
    pub provider_key: ProviderKeyInput,
}

/// The active provider's name plus its secret material.
#[derive(Debug, Clone)]
pub struct ProviderKeyInput {
    /// Provider name, e.g. `anthropic` / `deepseek` / `chatgpt`.
    pub provider: String,
    /// The secret to store under `[agent.providers.<provider>]`.
    pub secret: ProviderSecretInput,
}

/// Builds a [`Config`] from collected answers. Pure: no I/O, no validation —
/// the gate is [`write_config`]'s round-trip. Unset inputs are omitted so the
/// daemon's built-in defaults apply.
pub fn build_config(inputs: &ConfigInputs) -> Config {
    let forge = ForgeConfig {
        kind: inputs.forge_kind.clone(),
        url: inputs.forge_url.clone(),
        admin: inputs.admin_user.clone(),
        ci_user: inputs.ci_user.clone(),
    };

    let engine = EngineConfig {
        bind: inputs.webhook_addr.clone(),
        port: None,
        workflow: inputs.workflow_path.clone(),
        repos: non_empty_vec(&inputs.repos),
        roles: non_empty_vec(&inputs.roles),
        poll_cadence_secs: None,
        mechanical_cadence_secs: None,
        lease_ttl_secs: None,
        daemon_id: None,
        forge_token: None,
        webhook_secret: None,
        webhook_secret_file: None,
    };

    let worker = WorkerConfig {
        workspace: inputs.workspace.clone(),
        ..WorkerConfig::default()
    };

    let agent = AgentConfig {
        provider: inputs.provider.clone(),
        providers: if let (Some(provider_name), Some(url)) =
            (&inputs.provider, &inputs.provider_url)
        {
            let mut map = BTreeMap::new();
            map.insert(
                provider_name.clone(),
                AgentProviderConfig {
                    url: Some(url.clone()),
                    ..Default::default()
                },
            );
            map
        } else {
            BTreeMap::new()
        },
        ..AgentConfig::default()
    };

    Config {
        schema_version: SCHEMA_VERSION,
        deployment: DeploymentConfig::default(),
        workflow: WorkflowConfig::default(),
        paths: PathsConfig::default(),
        forge,
        engine,
        worker,
        agent,
    }
}

/// Builds a [`Credentials`] document from collected answers. Pure: no I/O.
pub fn build_credentials(inputs: &CredentialInputs) -> Credentials {
    let provider_credential = match &inputs.provider_key.secret {
        ProviderSecretInput::OAuth {
            access,
            refresh,
            expires,
        } => ProviderCredential {
            kind: Some("oauth".to_string()),
            access: Some(access.clone()),
            refresh: refresh.clone(),
            expires: Some(*expires),
            key: None,
            auth_file: None,
        },
        ProviderSecretInput::ApiKey(key) => ProviderCredential {
            kind: Some("api-key".to_string()),
            access: None,
            refresh: None,
            expires: None,
            key: Some(key.clone()),
            auth_file: None,
        },
        ProviderSecretInput::AuthFile(path) => ProviderCredential {
            kind: None,
            access: None,
            refresh: None,
            expires: None,
            key: None,
            auth_file: Some(path.clone()),
        },
    };

    let mut providers = BTreeMap::new();
    providers.insert(inputs.provider_key.provider.clone(), provider_credential);

    Credentials {
        schema_version: SCHEMA_VERSION,
        forge: ForgeCredentials {
            users: inputs.forge_users.clone(),
        },
        agent: AgentCredentials { providers },
        ..Credentials::default()
    }
}

/// Maps a set of provisioned forge identities into the `[forge.users.*]` table
/// the credentials file stores.
///
/// This is the **provision → credentials seam**: the caller (issue #182's
/// `temper init`) collects the provisioner's per-role result — keyed by role/user
/// name — into [`ProvisionedForgeUser`]s and hands them here. Because the input
/// type lives in this crate and carries only plain data, `temper-config` never
/// links `temper-provision`. A `user` that equals its key is dropped
/// (the key is the default) so the written file stays minimal.
pub fn forge_users_from_provisioned(
    provisioned: &BTreeMap<String, ProvisionedForgeUser>,
) -> BTreeMap<String, ForgeUser> {
    provisioned
        .iter()
        .map(|(key, identity)| {
            let user = identity.user.as_ref().filter(|user| *user != key).cloned();
            (
                key.clone(),
                ForgeUser {
                    user,
                    email: identity.email.clone(),
                    password: identity.password.clone(),
                    token: identity.token.clone(),
                },
            )
        })
        .collect()
}

/// Serializes, **re-validates**, then writes a config document to `path`.
///
/// The document is serialized to TOML and re-parsed through [`Config::parse`]
/// (the same loader the daemon uses) before anything is written, so a malformed
/// in-memory document fails with a [`ConfigError`] and leaves the disk
/// untouched. With `force == false` an existing `path` is an error; with
/// `force == true` it is overwritten. Parent directories are created as needed.
pub fn write_config(doc: &Config, path: &Path, force: bool) -> Result<(), ConfigError> {
    let text = serialize_validated(doc, FileKind::Config, |text| {
        Config::parse(text, path, FileKind::Config).map(|_| ())
    })?;
    write_atomic(&text, path, FileKind::Config, force, false)
}

/// Serializes, **re-validates**, then writes a credentials document to `path`,
/// `chmod 600` on Unix.
///
/// Same round-trip guarantee as [`write_config`]; additionally the file is
/// created (and on overwrite, set) owner-read/write-only so secrets are not
/// world-readable.
pub fn write_credentials(doc: &Credentials, path: &Path, force: bool) -> Result<(), ConfigError> {
    let text = serialize_validated(doc, FileKind::Credentials, |text| {
        Credentials::parse(text, path, FileKind::Credentials).map(|_| ())
    })?;
    write_atomic(&text, path, FileKind::Credentials, force, true)
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn non_empty_vec(items: &[String]) -> Option<Vec<String>> {
    if items.is_empty() {
        None
    } else {
        Some(items.to_vec())
    }
}

/// Serializes `doc` to TOML and confirms it re-parses via `reparse`, returning
/// the validated text. Any serialize/parse failure is surfaced *before* a write.
fn serialize_validated<T: serde::Serialize>(
    doc: &T,
    kind: FileKind,
    reparse: impl FnOnce(&str) -> Result<(), ConfigError>,
) -> Result<String, ConfigError> {
    let text = toml::to_string(doc).map_err(|error| ConfigError::Serialize {
        kind,
        message: error.to_string(),
    })?;
    reparse(&text)?;
    Ok(text)
}

/// Writes `text` to `path`, honoring `force` and (when `secret`) `0600`.
fn write_atomic(
    text: &str,
    path: &Path,
    kind: FileKind,
    force: bool,
    secret: bool,
) -> Result<(), ConfigError> {
    let write_err = |source: std::io::Error| ConfigError::Write {
        kind,
        path: path.to_path_buf(),
        source,
    };

    if !force && path.exists() {
        return Err(write_err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "file already exists (pass force to overwrite)",
        )));
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(write_err)?;
    }

    write_file(text, path, secret).map_err(write_err)
}

#[cfg(unix)]
fn write_file(text: &str, path: &Path, secret: bool) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    if secret {
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(text.as_bytes())?;
    // An existing file keeps its old mode through `OpenOptions`, so re-assert
    // `0600` on overwrite to guarantee the invariant regardless of prior perms.
    if secret {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn write_file(text: &str, path: &Path, _secret: bool) -> std::io::Result<()> {
    std::fs::write(path, text)
}

#[cfg(test)]
mod tests;

/// Convenience constructor: a [`ForgeUser`] with all fields. Re-exported helper
/// for callers assembling [`CredentialInputs::forge_users`] by hand (tests, and
/// `temper init` when not coming straight from a provisioning run).
pub fn forge_user(
    user: Option<String>,
    email: Option<String>,
    password: Option<String>,
    token: Option<String>,
) -> ForgeUser {
    ForgeUser {
        user,
        email,
        password,
        token,
    }
}

/// The default config-file path (`<config-dir>/config.toml`), if a config dir
/// can be determined. A convenience for `temper init`'s "where do I write?".
///
/// Binary-only shim: it snapshots the real environment ([`PathResolver::from_system`]
/// + [`SystemEnv`]). Hermetic callers should resolve via
/// [`crate::config_path`] with an explicit [`PathResolver`] + [`EnvLookup`].
#[doc(hidden)]
pub fn default_config_path() -> Option<PathBuf> {
    let paths = crate::PathResolver::from_system();
    crate::paths::config_path(None, &paths, &crate::SystemEnv)
}

/// The default credentials-file path (`CREDENTIALS_DIRECTORY/credentials.toml`
/// when set in the process environment, otherwise `<config-dir>/credentials.toml`).
///
/// Binary-only shim; see [`default_config_path`].
#[doc(hidden)]
pub fn default_credentials_path() -> Option<PathBuf> {
    let paths = crate::PathResolver::from_system();
    crate::paths::credentials_path(None, &paths, &crate::SystemEnv)
}
