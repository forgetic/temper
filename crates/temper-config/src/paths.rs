// SPDX-License-Identifier: MPL-2.0

//! Default on-disk locations for the config and credentials files.
//!
//! Resolution order for each: an explicit path (the `--config` / `--credentials`
//! flag) wins; then `<config-dir>/temper/{config,credentials}.toml`, where the
//! config dir is `$XDG_CONFIG_HOME` or `~/.config`. No environment variable
//! overrides these file locations.
//!
//! Every function here takes its inputs explicitly — a [`PathResolver`] for the
//! base directories and an injected [`EnvLookup`] seam — so this module reads no
//! ambient process environment. The binary boundary snapshots the real
//! environment via [`PathResolver::from_system`].

use std::path::PathBuf;

use crate::env::EnvLookup;
use crate::inputs::PathResolver;

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
/// `env` is threaded only so the path layer keeps a uniform injected-environment
/// seam (for `$HOME` / `$XDG_*` base-dir derivation upstream); no environment
/// variable overrides the config-file location.
pub fn config_path(
    explicit: Option<PathBuf>,
    paths: &PathResolver,
    _env: &dyn EnvLookup,
) -> Option<PathBuf> {
    explicit.or_else(|| config_dir(paths).map(|dir| dir.join("config.toml")))
}

/// Resolves the credentials-file path: explicit override (the `--credentials`
/// flag), else `<config-dir>/credentials.toml`.
///
/// As with [`config_path`], `env` is threaded only to keep the injected-environment
/// seam; no environment variable overrides the credentials-file location.
pub fn credentials_path(
    explicit: Option<PathBuf>,
    paths: &PathResolver,
    _env: &dyn EnvLookup,
) -> Option<PathBuf> {
    explicit.or_else(|| config_dir(paths).map(|dir| dir.join("credentials.toml")))
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
