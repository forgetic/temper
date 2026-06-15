// SPDX-License-Identifier: MPL-2.0

//! Default on-disk locations for the config and credentials files.
//!
//! Resolution order for each: an explicit path (the `--config` / `--credentials`
//! flag) wins; then the `TEMPER_CONFIG` / `TEMPER_CREDENTIALS` environment
//! variable; then `<config-dir>/temper/{config,credentials}.toml`, where the
//! config dir is `$XDG_CONFIG_HOME` or `~/.config`.

use std::path::PathBuf;

/// The base `…/temper` configuration directory (`$XDG_CONFIG_HOME/temper` or
/// `~/.config/temper`).
pub fn config_dir() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        return Some(PathBuf::from(xdg).join("temper"));
    }
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(|home| PathBuf::from(home).join(".config").join("temper"))
}

/// Resolves the config-file path: explicit override, else `TEMPER_CONFIG`, else
/// `<config-dir>/config.toml`.
pub fn config_path(explicit: Option<PathBuf>) -> Option<PathBuf> {
    explicit
        .or_else(|| std::env::var_os("TEMPER_CONFIG").map(PathBuf::from))
        .or_else(|| config_dir().map(|dir| dir.join("config.toml")))
}

/// Resolves the credentials-file path: explicit override, else
/// `TEMPER_CREDENTIALS`, else `<config-dir>/credentials.toml`.
pub fn credentials_path(explicit: Option<PathBuf>) -> Option<PathBuf> {
    explicit
        .or_else(|| std::env::var_os("TEMPER_CREDENTIALS").map(PathBuf::from))
        .or_else(|| config_dir().map(|dir| dir.join("credentials.toml")))
}
