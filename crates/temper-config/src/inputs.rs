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
//! - [`LoadInputs`] — the explicit `--config` / `--secrets` overrides plus an
//!   injected [`EnvLookup`] and the [`PathResolver`].
//!
//! [`load_explicit`] takes only [`LoadInputs`] and never touches `std::env`. The
//! **hermeticity contract** is: a `PathResolver` with every field `None` and an
//! empty [`EnvLookup`] discovers nothing — only explicit `--config` /
//! explicit `--secrets` paths and explicit-config sibling credentials load. Construct
//! an empty `PathResolver` in-memory (its `Default`) for fully isolated loads.

use std::path::{Path, PathBuf};

use crate::env::{EnvLookup, SystemEnv};
use crate::error::{ConfigError, FileKind};
use crate::resolved::Resolved;
use crate::schema::{Config, Credentials};
use crate::{LoadedPaths, paths};
use crate::resolve::{self, ResolveOptions};

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
    /// Derives the base directories from an injected [`EnvLookup`].
    ///
    /// Reads `XDG_CONFIG_HOME` / `XDG_STATE_HOME` / `HOME` through `env` (empty
    /// values are treated as unset). This is the hermetic constructor: hand it a
    /// test [`EnvMap`](crate::EnvMap) and discovery follows that snapshot; hand
    /// it [`SystemEnv`] (as [`from_system`](Self::from_system) does) and it
    /// snapshots the real process environment. With an empty `env` every field
    /// is `None`, so no default locations are discovered.
    pub fn from_env(env: &dyn EnvLookup) -> Self {
        let non_empty = |key: &str| env.non_empty(key).map(PathBuf::from);
        Self {
            xdg_config_home: non_empty("XDG_CONFIG_HOME"),
            xdg_state_home: non_empty("XDG_STATE_HOME"),
            home: non_empty("HOME"),
        }
    }

    /// Snapshots the real process environment's base directories.
    ///
    /// Binary-only: this is the explicit `std::env` boundary, expressed as
    /// [`from_env`](Self::from_env) over [`SystemEnv`]. The daemon's composition
    /// root (`src/bin/*`, `service_main`) builds one of these once; everything
    /// downstream takes the resulting plain data. Tests must build a
    /// `PathResolver` in-memory (via [`from_env`](Self::from_env) over an
    /// [`EnvMap`](crate::EnvMap), or `Default`) instead so they never discover
    /// the global config.
    #[doc(hidden)]
    pub fn from_system() -> Self {
        Self::from_env(&SystemEnv)
    }
}

/// Everything [`load_explicit`] needs to locate, read, and resolve the two
/// files, with no ambient environment access.
pub struct LoadInputs<'a> {
    /// Explicit `--config` path (wins over env + defaults).
    pub explicit_config: Option<PathBuf>,
    /// Explicit `--secrets` path (wins over env + defaults).
    pub explicit_credentials: Option<PathBuf>,
    /// Injected environment snapshot. Used only for `$HOME` / `$XDG_*` path
    /// expansion during resolution; no environment variable selects which files
    /// load or overrides their resolved deployment values.
    pub env: &'a dyn EnvLookup,
    /// Injected base directories for default-location discovery.
    pub paths: &'a PathResolver,
}

/// Loads + resolves the deployment from fully injected inputs.
///
/// Resolution per file: explicit override → `<config-dir>/{config,credentials}.toml`
/// derived from `inputs.paths`. An explicit path is *required* (a missing file
/// errors); a default-location file is *optional* (absent means built-in
/// defaults supply everything). No environment variable selects the files.
///
/// When `explicit_config` is set and `explicit_credentials` is not, the loader
/// reads sibling `<config-root>/credentials.toml` if it exists. That local-bundle
/// pairing is still hermetic: it does not fall through to the user's default
/// `~/.config/temper/credentials.toml` behind an explicit config.
///
/// Hermeticity: with `inputs.paths` empty (every field `None`), only explicit
/// paths and explicit-config sibling credentials can load — nothing is
/// discovered from the real environment.
pub fn load_explicit(inputs: &LoadInputs) -> Result<(Resolved, LoadedPaths), ConfigError> {
    let explicit_config = inputs
        .explicit_config
        .clone()
        .map(paths::explicit_config_location);
    let config_source = explicit_config
        .as_ref()
        .map(|location| LocatedFile::required(location.path.clone()))
        .or_else(|| {
            paths::config_dir(inputs.paths)
                .map(|dir| LocatedFile::optional(dir.join(paths::CONFIG_FILE_NAME)))
        });
    let credentials_source = inputs
        .explicit_credentials
        .clone()
        .map(paths::explicit_credentials_path)
        .map(LocatedFile::required)
        .or_else(|| {
            explicit_config.as_ref().map(|location| {
                LocatedFile::optional(location.root.join(paths::CREDENTIALS_FILE_NAME))
            })
        })
        .or_else(|| {
            paths::config_dir(inputs.paths)
                .map(|dir| LocatedFile::optional(dir.join(paths::CREDENTIALS_FILE_NAME)))
        });

    let (config, config_file) = load_optional(config_source, FileKind::Config, Config::parse)?;
    let (credentials, credentials_file) = load_optional(
        credentials_source,
        FileKind::Credentials,
        Credentials::parse,
    )?;

    let resolve_options = config_file
        .as_deref()
        .and_then(config_base_dir)
        .map(ResolveOptions::from_config_base_dir)
        .unwrap_or_default();
    let resolved = resolve::resolve_with_options(&config, &credentials, &inputs.env, &resolve_options)?;
    Ok((
        resolved,
        LoadedPaths {
            config: config_file,
            credentials: credentials_file,
        },
    ))
}

fn config_base_dir(path: &Path) -> Option<PathBuf> {
    path.parent().map(Path::to_path_buf)
}

#[derive(Debug, Clone)]
struct LocatedFile {
    path: PathBuf,
    required: bool,
}

impl LocatedFile {
    fn required(path: PathBuf) -> Self {
        Self {
            path,
            required: true,
        }
    }

    fn optional(path: PathBuf) -> Self {
        Self {
            path,
            required: false,
        }
    }
}

/// Reads + parses a located file, or returns a defaulted value when an optional
/// file is absent. The only sources are `inputs`: no `std::env` access, and no
/// environment variable overrides the file location.
fn load_optional<T: Default>(
    source: Option<LocatedFile>,
    kind: FileKind,
    parse: impl Fn(&str, &Path, FileKind) -> Result<T, ConfigError>,
) -> Result<(T, Option<PathBuf>), ConfigError> {
    // An explicit override is *required*: a missing file is an error. A
    // default-location or explicit-config sibling file is *optional*: absent
    // means built-in defaults.
    let Some(LocatedFile { path, required }) = source else {
        return Ok((T::default(), None));
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
