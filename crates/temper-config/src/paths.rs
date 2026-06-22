// SPDX-License-Identifier: MPL-2.0

//! Default on-disk locations for the config and credentials files.
//!
//! Resolution order for each: an explicit path (the `--config` / `--secrets`
//! flag) wins; then
//! `<config-dir>/temper/{config,credentials}.toml`, where the config dir is
//! `$XDG_CONFIG_HOME` or `~/.config`. No environment variable overrides these
//! file locations.
//!
//! Explicit file flags also accept a directory: `--config <dir>` resolves to
//! `<dir>/config.toml`, and `--secrets <dir>` resolves to
//! `<dir>/credentials.toml`. When the explicit path does not exist yet, a
//! `.toml` suffix is treated as a file and any other path is treated as a
//! directory; this lets `temper --config ./bundle init` create a new local
//! bundle directory.
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

/// A resolved explicit `--config` source.
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct ConfigLocation {
    /// The TOML file to read or write.
    pub path: PathBuf,
    /// The local bundle root used for sibling files such as `credentials.toml`.
    pub root: PathBuf,
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

/// Resolves the credentials-file path: explicit override (the `--secrets` flag),
/// else `<config-dir>/credentials.toml`.
///
/// An explicit directory resolves to `<dir>/credentials.toml`; an explicit file
/// path resolves to that exact file.
///
/// As with [`config_path`], `env` is threaded only to keep the injected-environment
/// seam; no environment variable overrides the credentials-file location.
pub fn credentials_path(
    explicit: Option<PathBuf>,
    paths: &PathResolver,
    _env: &dyn EnvLookup,
) -> Option<PathBuf> {
    explicit
        .map(explicit_credentials_path)
        .or_else(|| config_dir(paths).map(|dir| dir.join(CREDENTIALS_FILE_NAME)))
}

/// Resolves credentials using local-bundle pairing: an explicit credentials /
/// secrets path wins; otherwise an explicit config root contributes its sibling
/// `credentials.toml`; otherwise the default config directory is used.
pub fn paired_credentials_path(
    explicit_credentials: Option<PathBuf>,
    explicit_config: Option<PathBuf>,
    paths: &PathResolver,
    _env: &dyn EnvLookup,
) -> Option<PathBuf> {
    explicit_credentials
        .map(explicit_credentials_path)
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

    let root = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(PathBuf::new);
    ConfigLocation { path, root }
}

pub(crate) fn explicit_credentials_path(path: PathBuf) -> PathBuf {
    if is_directory_source(&path) {
        path.join(CREDENTIALS_FILE_NAME)
    } else {
        path
    }
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
/// State is mutable, machine-local data the daemon owns at runtime (the
/// per-job worker workspaces live under [`default_workspace_root`]). It is
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
    use crate::NoEnv;

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
    fn explicit_credentials_directory_resolves_to_credentials_toml() {
        let dir = scratch("credentials-dir");

        assert_eq!(
            explicit_credentials_path(dir.clone()),
            dir.join(CREDENTIALS_FILE_NAME)
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn explicit_credentials_file_resolves_to_exact_file() {
        let dir = scratch("credentials-file");
        let file = dir.join("local-secrets.toml");
        std::fs::write(&file, "schema_version = 1\n").expect("write file");

        assert_eq!(explicit_credentials_path(file.clone()), file);
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
