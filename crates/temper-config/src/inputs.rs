// SPDX-License-Identifier: MPL-2.0

//! Injectable inputs for config + credentials loading.
//!
//! `load` historically reached straight into `std::env` / `$HOME` / `$XDG_*` to
//! discover the default `~/.config/temper` locations. That made every call to
//! `load` with default options non-hermetic: any process — a test, a service —
//! would discover the operator's global config regardless of intent.
//!
//! This module is the structural fix. All of "where do the files live?" is
//! captured in two plain-data inputs:
//!
//! - [`PathResolver`] — the home / XDG base directories, normally a snapshot of
//!   the real environment built at the binary boundary ([`PathResolver::from_system`]);
//! - [`LoadInputs`] — the explicit `--config` / `--credentials` overrides plus
//!   an injected [`EnvLookup`] and the [`PathResolver`].
//!
//! [`load_explicit`] takes only [`LoadInputs`] and never touches `std::env`. The
//! **hermeticity contract** is: a `PathResolver` with every field `None` and an
//! empty [`EnvLookup`] discovers nothing — only explicit `--config` /
//! `--credentials` paths load. Construct an empty `PathResolver` in-memory (its
//! `Default`) for fully isolated loads.

use std::path::{Path, PathBuf};

use crate::env::EnvLookup;
use crate::error::{ConfigError, FileKind};
use crate::resolved::Resolved;
use crate::schema::{Config, Credentials};
use crate::{LoadedPaths, paths, resolve};

/// The base directories used to derive default file locations.
///
/// These are the only "ambient" inputs `load` ever needed. Capturing them as
/// plain data — built once at the binary boundary via [`PathResolver::from_system`]
/// — lets the loader stay hermetic: a default-constructed `PathResolver` (every
/// field `None`) resolves to no default locations at all.
#[derive(Debug, Clone, Default)]
pub struct PathResolver {
    /// `$XDG_CONFIG_HOME`, if set and non-empty.
    pub xdg_config_home: Option<PathBuf>,
    /// `$XDG_STATE_HOME`, if set and non-empty.
    pub xdg_state_home: Option<PathBuf>,
    /// `$HOME`, if set and non-empty.
    pub home: Option<PathBuf>,
}

impl PathResolver {
    /// Snapshots the real process environment's base directories.
    ///
    /// Binary-only: this is the explicit `std::env` boundary. The daemon's
    /// composition root (`src/bin/*`, `service_main`) builds one of these once;
    /// everything downstream takes the resulting plain data. Tests must build a
    /// `PathResolver` in-memory instead so they never discover the global config.
    #[doc(hidden)]
    pub fn from_system() -> Self {
        let non_empty = |key: &str| {
            std::env::var_os(key)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        Self {
            xdg_config_home: non_empty("XDG_CONFIG_HOME"),
            xdg_state_home: non_empty("XDG_STATE_HOME"),
            home: non_empty("HOME"),
        }
    }
}

/// Everything [`load_explicit`] needs to locate, read, and resolve the two
/// files, with no ambient environment access.
pub struct LoadInputs<'a> {
    /// Explicit `--config` path (wins over env + defaults).
    pub explicit_config: Option<PathBuf>,
    /// Explicit `--credentials` path (wins over env + defaults).
    pub explicit_credentials: Option<PathBuf>,
    /// Injected environment snapshot (incl. `TEMPER_CONFIG`, `TEMPER_CREDENTIALS`).
    pub env: &'a dyn EnvLookup,
    /// Injected base directories for default-location discovery.
    pub paths: &'a PathResolver,
}

/// Loads + resolves the deployment from fully injected inputs.
///
/// Resolution per file: explicit override → `TEMPER_CONFIG` /
/// `TEMPER_CREDENTIALS` from `inputs.env` → `<config-dir>/{config,credentials}.toml`
/// derived from `inputs.paths`. An explicit/env path is *required* (a missing
/// file errors); a default-location file is *optional* (absent means
/// env + built-in defaults supply everything).
///
/// Hermeticity: with `inputs.paths` empty (every field `None`) and an empty
/// `inputs.env`, only explicit paths can load — nothing is discovered from the
/// real environment.
pub fn load_explicit(inputs: &LoadInputs) -> Result<(Resolved, LoadedPaths), ConfigError> {
    let (config, config_file) = load_optional(
        inputs.explicit_config.clone(),
        "TEMPER_CONFIG",
        |dir| dir.join("config.toml"),
        FileKind::Config,
        inputs,
        Config::parse,
    )?;
    let (credentials, credentials_file) = load_optional(
        inputs.explicit_credentials.clone(),
        "TEMPER_CREDENTIALS",
        |dir| dir.join("credentials.toml"),
        FileKind::Credentials,
        inputs,
        Credentials::parse,
    )?;

    let resolved = resolve::resolve(&config, &credentials, &inputs.env)?;
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
/// file is absent. The only sources are `inputs`: no `std::env` access.
fn load_optional<T: Default>(
    explicit: Option<PathBuf>,
    env_key: &str,
    default_name: impl Fn(&Path) -> PathBuf,
    kind: FileKind,
    inputs: &LoadInputs,
    parse: impl Fn(&str, &Path, FileKind) -> Result<T, ConfigError>,
) -> Result<(T, Option<PathBuf>), ConfigError> {
    // An explicit override or an env-var path is *required*: a missing file is an
    // error. A default-location file is *optional*: absent means env+defaults.
    let (path, required) = match explicit {
        Some(path) => (path, true),
        None => match inputs.env.non_empty(env_key) {
            Some(path) => (PathBuf::from(path), true),
            None => match paths::config_dir(inputs.paths) {
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

#[cfg(test)]
mod tests;
