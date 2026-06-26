// SPDX-License-Identifier: MPL-2.0

use std::path::PathBuf;

/// Context for resolving path values that came from an on-disk config file.
///
/// Direct callers that build a [`Config`] in memory can keep using [`resolve`],
/// which preserves the historical behavior: relative path strings stay relative
/// to the caller's process. Loaders that know which `config.toml` supplied the
/// values pass its parent directory here so relative config-file paths are
/// interpreted beside that file.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ResolveOptions {
    /// Directory containing the loaded config file, if there was one.
    pub config_base_dir: Option<PathBuf>,
    /// Whether secret-name references must exist in the selected secret source.
    ///
    /// Normal config loading validates references. Path-inspection commands set
    /// this to `false` so `temper config paths` can report locations even while
    /// an operator is still assembling the secret bundle.
    pub validate_secret_references: bool,
}

impl Default for ResolveOptions {
    fn default() -> Self {
        Self {
            config_base_dir: None,
            validate_secret_references: true,
        }
    }
}

impl ResolveOptions {
    /// Builds options that resolve relative config-file paths against `dir`.
    pub fn from_config_base_dir(dir: impl Into<PathBuf>) -> Self {
        Self {
            config_base_dir: Some(dir.into()),
            ..Self::default()
        }
    }
}
