// SPDX-License-Identifier: MPL-2.0

//! Default on-disk locations for the config and credentials files.
//!
//! Config resolution order: an explicit `--config` path wins, then
//! `<config-dir>/temper/config.toml`, where the config dir is `$XDG_CONFIG_HOME`
//! or `~/.config`.
//!
//! Credentials/secret source resolution order: an explicit `--secrets` source
//! wins; then systemd's `CREDENTIALS_DIRECTORY` contributes the directory as a
//! named-file secret source; then local-bundle pairing may use an explicit
//! config root's sibling `credentials.toml`; then the normal
//! `<config-dir>/temper/credentials.toml` default is used.
//!
//! Explicit config flags still accept a directory: `--config <dir>` resolves to
//! `<dir>/config.toml`. Explicit secret flags accept either a TOML file or a
//! directory secret source: an existing directory, or a missing path without a
//! `.toml` suffix, is treated as a directory source; a `.toml` path is treated
//! as a credentials TOML file.
//!
//! Every function here takes its inputs explicitly — a [`PathResolver`] for the
//! base directories and an injected [`EnvLookup`] seam — so this module reads no
//! ambient process environment. The binary boundary snapshots the real
//! environment via [`PathResolver::from_system`].

use std::path::{Path, PathBuf};

use crate::env::EnvLookup;
use crate::inputs::PathResolver;

pub(crate) const CONFIG_FILE_NAME: &str = "config.toml";
pub(crate) const CREDENTIALS_FILE_NAME: &str = "credentials.toml";
pub(crate) const CREDENTIALS_DIRECTORY_ENV: &str = "CREDENTIALS_DIRECTORY";

/// A resolved explicit `--config` source.
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct ConfigLocation {
    /// The TOML file to read or write.
    pub path: PathBuf,
    /// The local bundle root used for sibling files such as `credentials.toml`.
    pub root: PathBuf,
}

/// A resolved secret source.
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum SecretSourceLocation {
    /// A credentials TOML file.
    File(PathBuf),
    /// A directory whose regular files are named secrets.
    Directory(PathBuf),
}

impl SecretSourceLocation {
    pub(crate) fn into_path(self) -> PathBuf {
        match self {
            SecretSourceLocation::File(path) | SecretSourceLocation::Directory(path) => path,
        }
    }
}

/// The base `…/temper` configuration directory (`$XDG_CONFIG_HOME/temper` or
/// `~/.config/temper`), derived from the injected [`PathResolver`].
pub fn config_dir(paths: &PathResolver) -> Option<PathBuf> {
    if let Some(xdg) = &paths.xdg_config_home {
        return Some(xdg.join("temper"));
    }
    paths
        .home
        .as_ref()
        .map(|home| home.join(".config").join("temper"))
}

/// Resolves the config-file path: explicit override (the `--config` flag), else
/// `<config-dir>/config.toml`.
///
/// An explicit directory resolves to `<dir>/config.toml`; an explicit file path
/// resolves to that exact file.
///
/// `env` is threaded only so the path layer keeps a uniform injected-environment
/// seam (for `$HOME` / `$XDG_*` base-dir derivation upstream); no environment
/// variable overrides the config-file location.
pub fn config_path(
    explicit: Option<PathBuf>,
    paths: &PathResolver,
    _env: &dyn EnvLookup,
) -> Option<PathBuf> {
    explicit
        .map(|path| explicit_config_location(path).path)
        .or_else(|| config_dir(paths).map(|dir| dir.join(CONFIG_FILE_NAME)))
}

/// Resolves the selected secret source path: explicit override (the
/// `--secrets` flag), else systemd's `CREDENTIALS_DIRECTORY`, else
/// `<config-dir>/credentials.toml`.
///
/// A directory source is reported as the directory itself; a TOML file source is
/// reported as that file. This is the path-inspection/load model. Use
/// [`paired_credentials_file_path`] when resolving a write target for
/// `credentials.toml`.
pub fn credentials_path(
    explicit: Option<PathBuf>,
    paths: &PathResolver,
    env: &dyn EnvLookup,
) -> Option<PathBuf> {
    explicit
        .map(explicit_secret_source_location)
        .map(SecretSourceLocation::into_path)
        .or_else(|| credentials_directory_source_location(env).map(SecretSourceLocation::into_path))
        .or_else(|| config_dir(paths).map(|dir| dir.join(CREDENTIALS_FILE_NAME)))
}

/// Resolves the selected secret source using local-bundle pairing: an explicit
/// secret source wins; otherwise systemd's `CREDENTIALS_DIRECTORY` contributes
/// the directory; otherwise an explicit config root contributes its sibling
/// `credentials.toml`; otherwise the default config directory is used.
pub fn paired_credentials_path(
    explicit_credentials: Option<PathBuf>,
    explicit_config: Option<PathBuf>,
    paths: &PathResolver,
    env: &dyn EnvLookup,
) -> Option<PathBuf> {
    paired_secret_source_location(explicit_credentials, explicit_config, paths, env)
        .map(SecretSourceLocation::into_path)
}

/// Resolves the legacy credentials TOML write target. Directory-shaped explicit
/// `--secrets` paths resolve to `<dir>/credentials.toml`; `CREDENTIALS_DIRECTORY`
/// is intentionally ignored by callers that write local bundles.
pub fn paired_credentials_file_path(
    explicit_credentials: Option<PathBuf>,
    explicit_config: Option<PathBuf>,
    paths: &PathResolver,
) -> Option<PathBuf> {
    explicit_credentials
        .map(explicit_credentials_file_path)
        .or_else(|| {
            explicit_config
                .map(explicit_config_location)
                .map(|location| location.root.join(CREDENTIALS_FILE_NAME))
        })
        .or_else(|| config_dir(paths).map(|dir| dir.join(CREDENTIALS_FILE_NAME)))
}

pub(crate) fn explicit_config_location(path: PathBuf) -> ConfigLocation {
    if is_directory_source(&path) {
        return ConfigLocation {
            path: path.join(CONFIG_FILE_NAME),
            root: path,
        };
    }

    let root = path.parent().map(Path::to_path_buf).unwrap_or_default();
    ConfigLocation { path, root }
}

pub(crate) fn explicit_secret_source_location(path: PathBuf) -> SecretSourceLocation {
    if is_directory_source(&path) {
        SecretSourceLocation::Directory(path)
    } else {
        SecretSourceLocation::File(path)
    }
}

pub(crate) fn explicit_credentials_file_path(path: PathBuf) -> PathBuf {
    if is_directory_source(&path) {
        path.join(CREDENTIALS_FILE_NAME)
    } else {
        path
    }
}

pub(crate) fn credentials_directory_source_location(
    env: &dyn EnvLookup,
) -> Option<SecretSourceLocation> {
    env.non_empty(CREDENTIALS_DIRECTORY_ENV)
        .map(PathBuf::from)
        .map(SecretSourceLocation::Directory)
}

pub(crate) fn paired_secret_source_location(
    explicit_credentials: Option<PathBuf>,
    explicit_config: Option<PathBuf>,
    paths: &PathResolver,
    env: &dyn EnvLookup,
) -> Option<SecretSourceLocation> {
    explicit_credentials
        .map(explicit_secret_source_location)
        .or_else(|| credentials_directory_source_location(env))
        .or_else(|| {
            explicit_config
                .map(explicit_config_location)
                .map(|location| {
                    SecretSourceLocation::File(location.root.join(CREDENTIALS_FILE_NAME))
                })
        })
        .or_else(|| {
            config_dir(paths).map(|dir| SecretSourceLocation::File(dir.join(CREDENTIALS_FILE_NAME)))
        })
}

fn is_directory_source(path: &Path) -> bool {
    match std::fs::metadata(path) {
        Ok(metadata) => metadata.is_dir(),
        Err(_) => path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_none_or(|extension| !extension.eq_ignore_ascii_case("toml")),
    }
}

/// The base `…/temper` **state** directory (`$XDG_STATE_HOME/temper` or
/// `~/.local/state/temper`), per the XDG Base Directory spec, derived from the
/// injected [`PathResolver`].
///
/// State is mutable, machine-local data the daemon owns at runtime (the worker's
/// top-level workspace root lives under [`default_workspace_root`]). It is
/// deliberately separate from the *config* dir ([`config_dir`]): config is
/// hand-edited and may be checked in, state is generated and disposable.
pub fn state_dir(paths: &PathResolver) -> Option<PathBuf> {
    if let Some(xdg) = &paths.xdg_state_home {
        return Some(xdg.join("temper"));
    }
    paths
        .home
        .as_ref()
        .map(|home| home.join(".local").join("state").join("temper"))
}

/// The default worker workspace root: `<state-dir>/workspace`, where the state
/// dir is `$XDG_STATE_HOME/temper` or `~/.local/state/temper`.
///
/// `None` only when neither `$XDG_STATE_HOME` nor `$HOME` is set in the injected
/// [`PathResolver`]; the resolver then falls back to a relative literal (see
/// [`crate::resolve`]).
pub fn default_workspace_root(paths: &PathResolver) -> Option<PathBuf> {
    state_dir(paths).map(|dir| dir.join("workspace"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EnvMap, NoEnv};

    fn scratch(tag: &str) -> PathBuf {
        let pid = std::process::id();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("temper-config-paths-{tag}-{pid}-{nonce}"));
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    #[test]
    fn explicit_config_directory_resolves_to_config_toml_and_root() {
        let dir = scratch("config-dir");
        let location = explicit_config_location(dir.clone());

        assert_eq!(location.path, dir.join(CONFIG_FILE_NAME));
        assert_eq!(location.root, dir);
        let _ = std::fs::remove_dir_all(location.root);
    }

    #[test]
    fn explicit_config_file_resolves_to_exact_file_and_parent_root() {
        let dir = scratch("config-file");
        let file = dir.join("local-dev.toml");
        std::fs::write(&file, "schema_version = 1\n").expect("write file");

        let location = explicit_config_location(file.clone());

        assert_eq!(location.path, file);
        assert_eq!(location.root, dir);
        let _ = std::fs::remove_dir_all(location.root);
    }

    #[test]
    fn missing_extensionless_config_path_is_a_new_bundle_directory() {
        let dir = scratch("missing-config-dir");
        let bundle = dir.join("bundle");

        let location = explicit_config_location(bundle.clone());

        assert_eq!(location.path, bundle.join(CONFIG_FILE_NAME));
        assert_eq!(location.root, bundle);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn explicit_credentials_directory_resolves_to_directory_source() {
        let dir = scratch("credentials-dir");

        assert_eq!(
            explicit_secret_source_location(dir.clone()),
            SecretSourceLocation::Directory(dir.clone())
        );
        assert_eq!(
            explicit_credentials_file_path(dir.clone()),
            dir.join(CREDENTIALS_FILE_NAME)
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn explicit_credentials_file_resolves_to_exact_file() {
        let dir = scratch("credentials-file");
        let file = dir.join("local-secrets.toml");
        std::fs::write(&file, "schema_version = 1\n").expect("write file");

        assert_eq!(
            explicit_secret_source_location(file.clone()),
            SecretSourceLocation::File(file.clone())
        );
        assert_eq!(explicit_credentials_file_path(file.clone()), file);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn credentials_directory_resolves_to_directory_source_before_default() {
        let credentials_dir = scratch("systemd-credentials");
        let xdg = scratch("systemd-credentials-xdg");
        let mut env = EnvMap::new();
        env.insert(
            CREDENTIALS_DIRECTORY_ENV,
            credentials_dir.to_string_lossy().into_owned(),
        );
        let paths = PathResolver {
            xdg_config_home: Some(xdg.clone()),
            ..PathResolver::default()
        };

        assert_eq!(
            credentials_path(None, &paths, &env),
            Some(credentials_dir.clone())
        );
        let _ = std::fs::remove_dir_all(credentials_dir);
        let _ = std::fs::remove_dir_all(xdg);
    }

    #[test]
    fn paired_credentials_uses_credentials_directory_before_sibling() {
        let dir = scratch("paired-systemd");
        let config_file = dir.join("deploy.toml");
        let credentials_dir = dir.join("systemd-creds");
        std::fs::create_dir_all(&credentials_dir).expect("create credentials dir");
        let mut env = EnvMap::new();
        env.insert(
            CREDENTIALS_DIRECTORY_ENV,
            credentials_dir.to_string_lossy().into_owned(),
        );

        assert_eq!(
            paired_credentials_path(None, Some(config_file), &PathResolver::default(), &env),
            Some(credentials_dir.clone())
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn paired_credentials_uses_explicit_config_root_when_no_secret_source() {
        let dir = scratch("paired");
        let config_file = dir.join("deploy.toml");

        assert_eq!(
            paired_credentials_path(
                Some(dir.join("secrets.toml")),
                Some(config_file.clone()),
                &PathResolver::default(),
                &NoEnv
            ),
            Some(dir.join("secrets.toml"))
        );
        assert_eq!(
            paired_credentials_path(None, Some(config_file), &PathResolver::default(), &NoEnv),
            Some(dir.join(CREDENTIALS_FILE_NAME))
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
