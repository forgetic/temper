// SPDX-License-Identifier: MPL-2.0

//! Schema-based configuration for the temper deployment.
//!
//! Two TOML files, both stamped with the same `schema_version`:
//!
//! - the **config** file ([`Config`]) — non-secret deployment settings;
//! - the **credentials** file ([`Credentials`]) — secrets.
//!
//! [`load`] reads both (honoring `--config` / `--credentials` overrides, the
//! `TEMPER_CONFIG` / `TEMPER_CREDENTIALS` environment, and default
//! `~/.config/temper` locations), validates each file's `schema_version`, then
//! [`resolve`](resolve::resolve)s everything — file, environment, and built-in
//! defaults — into a [`Resolved`] the binary's adapters turn into runtime types.
//!
//! This crate depends only on `serde` + `toml` + `thiserror`: it is
//! tier-agnostic so every binary (unified or slim per-service) can read config
//! without pulling in the engine/worker/agent stacks.

mod cli;
mod env;
mod error;
mod paths;
pub mod provider;
mod resolve;
mod resolved;
mod schema;
mod template;

use std::path::{Path, PathBuf};

pub use cli::{CommonArgs, parse_common_args};
pub use env::{EnvLookup, NoEnv, SystemEnv};
pub use error::{ConfigError, FileKind};
pub use paths::{config_dir, config_path, credentials_path};
pub use resolve::{env_role_key, resolve};
pub use resolved::{
    AgentSettings, Capability, EngineSettings, ForgeKind, ForgeSettings, GitIdentity,
    ProviderCredential, ProviderKind, ProviderSettings, RepoPath, Resolved, WebUiCreds,
    WorkerSettings,
};
pub use schema::{
    AgentConfig, AgentCredentials, AgentProviderConfig, Config, Credentials, EngineConfig,
    ForgeConfig, ForgeCredentials, ForgeUser, ModelMap, ProviderCredential as ProviderCredentialFile,
    SCHEMA_VERSION, WorkerConfig as WorkerFileConfig,
};
pub use template::{config_template, credentials_template};

/// Where to find the two files. `None` fields fall back to `TEMPER_CONFIG` /
/// `TEMPER_CREDENTIALS` and then the default `~/.config/temper` locations.
#[derive(Debug, Clone, Default)]
pub struct LoadOptions {
    /// Explicit `--config` path.
    pub config: Option<PathBuf>,
    /// Explicit `--credentials` path.
    pub credentials: Option<PathBuf>,
}

/// The files that actually fed a [`load`], for diagnostics (`None` = absent, so
/// defaults + environment supplied everything).
#[derive(Debug, Clone, Default)]
pub struct LoadedPaths {
    pub config: Option<PathBuf>,
    pub credentials: Option<PathBuf>,
}

/// Loads + resolves the deployment using the process environment.
pub fn load(options: &LoadOptions) -> Result<(Resolved, LoadedPaths), ConfigError> {
    load_with_env(options, &SystemEnv)
}

/// Loads + resolves with an explicit environment source (testable).
pub fn load_with_env(
    options: &LoadOptions,
    env: &impl EnvLookup,
) -> Result<(Resolved, LoadedPaths), ConfigError> {
    let (config, config_file) = load_optional(
        options.config.clone(),
        "TEMPER_CONFIG",
        |dir| dir.join("config.toml"),
        FileKind::Config,
        env,
        Config::parse,
    )?;
    let (credentials, credentials_file) = load_optional(
        options.credentials.clone(),
        "TEMPER_CREDENTIALS",
        |dir| dir.join("credentials.toml"),
        FileKind::Credentials,
        env,
        Credentials::parse,
    )?;

    let resolved = resolve::resolve(&config, &credentials, env)?;
    Ok((
        resolved,
        LoadedPaths {
            config: config_file,
            credentials: credentials_file,
        },
    ))
}

/// Locates a file (explicit override / env var = required; default location =
/// optional), reads + parses it, or returns a defaulted value when an optional
/// file is absent.
fn load_optional<T: Default>(
    explicit: Option<PathBuf>,
    env_key: &str,
    default_name: impl Fn(&Path) -> PathBuf,
    kind: FileKind,
    env: &impl EnvLookup,
    parse: impl Fn(&str, &Path, FileKind) -> Result<T, ConfigError>,
) -> Result<(T, Option<PathBuf>), ConfigError> {
    // An explicit override or an env-var path is *required*: a missing file is an
    // error. A default-location file is *optional*: absent means env+defaults.
    let (path, required) = match explicit {
        Some(path) => (path, true),
        None => match env.non_empty(env_key) {
            Some(path) => (PathBuf::from(path), true),
            None => match paths::config_dir() {
                Some(dir) => (default_name(&dir), false),
                None => return Ok((T::default(), None)),
            },
        },
    };

    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let parsed = parse(&text, &path, kind)?;
            Ok((parsed, Some(path)))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !required => {
            Ok((T::default(), None))
        }
        Err(source) => Err(ConfigError::Read { kind, path, source }),
    }
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
/// Parses the common `--config` / `--credentials` / `--help` / `--version`
/// flags, loads + resolves the deployment from the process environment, and
/// hands the [`Resolved`] to `run`. This is the *entire* body of each slim
/// binary's `main` — the proof that a per-service binary needs no plumbing
/// beyond naming its service and its runner.
pub fn service_main(
    name: &str,
    usage: &str,
    args: impl IntoIterator<Item = String>,
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
    let (resolved, _paths) = match load(&parsed.options) {
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
/// `temper config validate` to print. Errors are blocking; notes are advisory
/// (e.g. a service that simply will not be used in this deployment).
pub fn lint(resolved: &Resolved) -> Vec<Finding> {
    let mut findings = Vec::new();

    if resolved.forge.url.is_none() {
        findings.push(Finding::error(
            "forge URL is unset (`[forge] url` or TEMPER_FORGE_URL / FORGEJO_URL)",
        ));
    }
    if resolved.forge.admin_token.is_none() {
        findings.push(Finding::error(
            "forge admin token is unset (`[forge.users.<admin>] token` or \
             TEMPER_FORGE_TOKEN / FORGEJO_ACCESS_TOKEN)",
        ));
    }
    if resolved.engine.repos.is_empty() {
        findings.push(Finding::error("no repositories configured (`[engine] repos`)"));
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
        if !resolved.forge.role_identities.contains_key(&capability.role) {
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
