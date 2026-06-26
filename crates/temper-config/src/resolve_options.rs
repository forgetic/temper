// SPDX-License-Identifier: MPL-2.0

use std::path::PathBuf;

/// Context for resolving path values that came from an on-disk config file.
///
/// Direct callers that build a [`Config`] in memory can keep using [`resolve`],
/// which preserves the historical behavior: relative path strings stay relative
/// to the caller's process. Loaders that know which `config.toml` supplied the
/// values pass its parent directory here so relative config-file paths are
/// interpreted beside that file.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct ResolveOptions {
    /// Directory containing the loaded config file, if there was one.
    pub config_base_dir: Option<PathBuf>,
}

impl ResolveOptions {
    /// Builds options that resolve relative config-file paths against `dir`.
    pub fn from_config_base_dir(dir: impl Into<PathBuf>) -> Self {
        Self {
            config_base_dir: Some(dir.into()),
        }
    }
}

