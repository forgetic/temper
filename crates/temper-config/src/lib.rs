// SPDX-License-Identifier: MPL-2.0

//! Schema-based configuration for the temper deployment.
//!
//! Two TOML files, both stamped with the same `schema_version`:
//!
//! - the **config** file ([`Config`]) — non-secret deployment settings;
//! - the **credentials** file ([`Credentials`]) — secrets.
//!
//! [`load`] reads both (honoring `--config` / `--secrets` overrides, systemd's
//! `CREDENTIALS_DIRECTORY` for credentials, and default `~/.config/temper`
//! locations), validates each file's `schema_version`, then
//! [`resolve`](resolve::resolve)s everything — file, environment, and built-in
//! defaults — into a [`Resolved`] the binary's adapters turn into runtime types.
//!
//! This crate depends only on `serde` + `toml` + `thiserror`: it is
//! tier-agnostic so every binary (unified or slim per-service) can read config
//! without pulling in the engine/worker/agent stacks.

mod build;
mod cli;
mod env;
mod error;
mod inputs;
mod json_schema;
mod paths;
pub mod provider;
mod resolve;
mod resolved;
mod schema;
mod template;

use std::path::{Path, PathBuf};

/// Crate-local alias for a type-level string secret.
///
/// Backed by [`secrecy::SecretString`]: `Debug` prints `[REDACTED]`, there is no
/// `Display`, it is not `Serialize` (so a secret cannot leak through a struct
/// dump or accidental serialization), and the buffer zeroizes on drop. Construct
/// one with `Secret::from("…")` and read the raw value at an I/O boundary with
/// [`ExposeSecret::expose_secret`]. For a non-string secret, use
/// [`secrecy::SecretBox`] directly.
pub type Secret = secrecy::SecretString;
pub use secrecy::ExposeSecret;

pub use build::{
    ConfigInputs, CredentialInputs, ProviderKeyInput, ProviderSecretInput, ProvisionedForgeUser,
    build_config, build_credentials, default_config_path, default_credentials_path, forge_user,
    forge_users_from_provisioned, write_config, write_credentials,
};
pub use cli::{CommonArgs, parse_common_args};
pub use env::{EnvLookup, EnvMap, NoEnv, SystemEnv};
pub use error::{ConfigError, FileKind};
pub use inputs::{LoadInputs, PathResolver, load_explicit};
pub use json_schema::config_json_schema;
pub use paths::{
    config_dir, config_path, credentials_path, default_workspace_root, paired_credentials_path,
    state_dir,
};
pub use resolve::{ResolveOptions, env_role_key, resolve, resolve_with_options};
pub use resolved::{
    AgentSettings, Capability, EngineSettings, ForgeKind, ForgeSettings, GitIdentity,
    ProviderCredential, ProviderKind, ProviderSettings, RepoPath, Resolved, WebUiCreds,
    WorkerSettings,
};
pub use schema::{
    AgentConfig, AgentCredentials, AgentProviderConfig, Config, Credentials, EngineConfig,
    ForgeConfig, ForgeCredentials, ForgeUser, ModelMap,
    ProviderCredential as ProviderCredentialFile, SCHEMA_VERSION, WorkerConfig as WorkerFileConfig,
};
pub use template::{config_template, credentials_template};

/// Where to find the two files. `None` config falls back to the default
/// `~/.config/temper` location. `None` credentials first checks the injected
/// `CREDENTIALS_DIRECTORY`, then falls back to an explicit config root's sibling
/// or the default `~/.config/temper` location.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct LoadOptions {
    /// Explicit `--config` path.
    pub config: Option<PathBuf>,
    /// Explicit `--secrets` path.
    pub credentials: Option<PathBuf>,
}

/// The files that actually fed a [`load`], for diagnostics (`None` = absent, so
/// defaults + environment supplied everything).
#[derive(Debug, Clone, Default)]
pub struct LoadedPaths {
    pub config: Option<PathBuf>,
    pub credentials: Option<PathBuf>,
}

/// Loads + resolves with an explicit environment source (testable seam).
///
/// Default-location discovery follows the **injected** `env`: the
/// [`PathResolver`] is built from it via [`PathResolver::from_env`], so a caller
/// whose `env` snapshot sets `HOME` / `XDG_CONFIG_HOME` still discovers
/// `~/.config/temper/{config,credentials}.toml`, exactly as before paths/env
/// were made injectable. An `env` that sets none of those discovers nothing.
///
/// For *strict* explicit-paths-only loads (the hermeticity contract) call
/// [`load_explicit`] with an empty [`PathResolver`] directly.
pub fn load_with_env(
    options: &LoadOptions,
    env: &impl EnvLookup,
) -> Result<(Resolved, LoadedPaths), ConfigError> {
    load_explicit(&LoadInputs {
        explicit_config: options.config.clone(),
        explicit_credentials: options.credentials.clone(),
        env,
        paths: &PathResolver::from_env(&env),
    })
}

impl Config {
    /// Reads and validates a config file from disk.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            kind: FileKind::Config,
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(&text, path, FileKind::Config)
    }

    /// Parses + validates config TOML, naming `path`/`kind` in any error.
    fn parse(text: &str, path: &Path, kind: FileKind) -> Result<Self, ConfigError> {
        check_schema_version(text, path, kind)?;
        toml::from_str(text).map_err(|error| ConfigError::Parse {
            kind,
            path: path.to_path_buf(),
            message: error.message().to_string(),
        })
    }
}

impl Credentials {
    /// Reads and validates a credentials file from disk.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            kind: FileKind::Credentials,
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(&text, path, FileKind::Credentials)
    }

    /// Parses + validates credentials TOML, naming `path`/`kind` in any error.
    fn parse(text: &str, path: &Path, kind: FileKind) -> Result<Self, ConfigError> {
        check_schema_version(text, path, kind)?;
        toml::from_str(text).map_err(|error| ConfigError::Parse {
            kind,
            path: path.to_path_buf(),
            message: error.message().to_string(),
        })
    }
}

/// Confirms the file declares a supported `schema_version` before the typed
/// parse, so a version mismatch gives a clear message instead of a field error.
fn check_schema_version(text: &str, path: &Path, kind: FileKind) -> Result<(), ConfigError> {
    let value: toml::Value = toml::from_str(text).map_err(|error| ConfigError::Parse {
        kind,
        path: path.to_path_buf(),
        message: error.message().to_string(),
    })?;
    let version = value.get("schema_version");
    let mismatch = |message: String| ConfigError::SchemaVersion {
        kind,
        path: path.to_path_buf(),
        message,
    };
    match version {
        None => Err(mismatch(format!(
            "missing `schema_version`; this build expects schema_version = {SCHEMA_VERSION}"
        ))),
        Some(toml::Value::Integer(found)) if *found == i64::from(SCHEMA_VERSION) => Ok(()),
        Some(toml::Value::Integer(found)) => Err(mismatch(format!(
            "unsupported schema_version {found}; this build expects {SCHEMA_VERSION}"
        ))),
        Some(other) => Err(mismatch(format!(
            "schema_version must be an integer, found {other}"
        ))),
    }
}

/// Exit code for a usage error, matching `EX_USAGE` from `sysexits.h`.
pub const EX_USAGE: u8 = 64;

/// The shared entry point for a slim per-service binary.
///
/// Parses the common `--config` / `--secrets` / `--help` / `--version` flags,
/// loads + resolves the deployment from the **injected** environment snapshot
/// (`env` / `paths`, captured by the binary's composition root), and hands the
/// [`Resolved`] to `run`. This is the *entire* body of each slim binary's
/// `main` — the proof that a per-service binary needs no plumbing beyond naming
/// its service, snapshotting its env, and naming its runner.
///
/// Hermeticity: an explicit `--config` / `--secrets` suppresses default
/// `~/.config/temper` discovery (an empty [`PathResolver`] is used).
/// `CREDENTIALS_DIRECTORY` from the injected env may still supply the credentials
/// file when `--secrets` is absent; otherwise an explicit config root may load
/// sibling `credentials.toml`, but the operator's global credentials never
/// ambiently layer in behind an explicit deployment — matching the unified
/// `temper daemon` path.
pub fn service_main(
    name: &str,
    usage: &str,
    args: impl IntoIterator<Item = String>,
    env: &impl EnvLookup,
    paths: &PathResolver,
    run: impl FnOnce(&Resolved) -> Result<(), String>,
) -> std::process::ExitCode {
    use std::process::ExitCode;

    let parsed = match parse_common_args(args) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("{name}: {error}\n\n{usage}");
            return ExitCode::from(EX_USAGE);
        }
    };
    if parsed.help {
        println!("{usage}");
        return ExitCode::SUCCESS;
    }
    if parsed.version {
        println!("{name} {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    if let Some(unknown) = parsed.rest.first() {
        eprintln!("{name}: unexpected argument `{unknown}`\n\n{usage}");
        return ExitCode::from(EX_USAGE);
    }
    let explicit = parsed.options.config.is_some() || parsed.options.credentials.is_some();
    let empty = PathResolver::default();
    let discovery: &PathResolver = if explicit { &empty } else { paths };
    let loaded = load_explicit(&LoadInputs {
        explicit_config: parsed.options.config.clone(),
        explicit_credentials: parsed.options.credentials.clone(),
        env,
        paths: discovery,
    });
    let (resolved, _paths) = match loaded {
        Ok(loaded) => loaded,
        Err(error) => {
            eprintln!("{name}: {error}");
            return ExitCode::FAILURE;
        }
    };
    match run(&resolved) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{name}: {error}");
            ExitCode::FAILURE
        }
    }
}

/// A human-readable finding from [`lint`].
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Finding {
    /// `true` for a blocking problem, `false` for an advisory note.
    pub error: bool,
    pub message: String,
}

impl Finding {
    fn error(message: impl Into<String>) -> Self {
        Self {
            error: true,
            message: message.into(),
        }
    }

    fn note(message: impl Into<String>) -> Self {
        Self {
            error: false,
            message: message.into(),
        }
    }
}

/// Audits a [`Resolved`] for the common requirements, returning findings for
/// `temper check` / `temper config validate` to print. Errors are blocking for
/// the compatibility validation path; notes are advisory (e.g. a service that
/// simply will not be used in this deployment).
pub fn lint(resolved: &Resolved) -> Vec<Finding> {
    let mut findings = Vec::new();

    if resolved.forge.url.is_none() {
        findings.push(Finding::error(
            "forge URL is unset (set `[forge] url` in temper.toml)",
        ));
    }
    if resolved.forge.admin_token.is_none() {
        findings.push(Finding::error(
            "forge admin token is unset (set a `token` under \
             `[forge.users.<admin>]` in credentials.toml, and name the admin \
             via `[forge] admin`)",
        ));
    }
    if resolved.engine.repos.is_empty() {
        findings.push(Finding::error(
            "no repositories configured (`[engine] repos`)",
        ));
    }
    if resolved.engine.roles.is_empty() {
        findings.push(Finding::error("no roles configured (`[engine] roles`)"));
    }
    if resolved.forge.web_ui.is_none() {
        findings.push(Finding::note(
            "no CI-reader web-UI credentials (`[forge] ci_user` + that user's \
             password); CI status reads will fail on a REST-less Forgejo (ADR 0019)",
        ));
    }

    // Every capability role should have a resolvable git identity (the worker
    // needs a push token), or it cannot run that role's jobs.
    for capability in &resolved.worker.capabilities {
        if !resolved
            .forge
            .role_identities
            .contains_key(&capability.role)
        {
            findings.push(Finding::error(format!(
                "role `{}` (capability `{}`) has no git identity: add \
                 `[forge.users.{}]` with a token to the credentials file",
                capability.role, capability.repo, capability.role
            )));
        }
    }

    if matches!(
        resolved.agent.provider.credential,
        ProviderCredential::Ambient
    ) {
        findings.push(Finding::note(format!(
            "agent provider `{}` has no credential in the credentials file \
             (`[agent.providers.{}]`); falling back to ambient auth (env / \
             ~/.pi/agent/auth.json)",
            resolved.agent.provider.kind.as_str(),
            resolved.agent.provider.kind.as_str()
        )));
    }

    findings
}

#[cfg(test)]
mod tests;
