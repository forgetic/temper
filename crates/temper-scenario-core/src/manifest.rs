// SPDX-License-Identifier: MPL-2.0

use std::fmt;
use std::io;
use std::path::PathBuf;

use thiserror::Error;

use crate::{Diagnostic, Severity};

/// Lifecycle status for a scenario manifest.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub enum ScenarioStatus {
    Draft,
    Ready,
    Active,
    Disabled,
    Deprecated,
    Archived,
    Retired,
}

impl ScenarioStatus {
    /// Parses a lowercase manifest status.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "draft" => Some(Self::Draft),
            "ready" => Some(Self::Ready),
            "active" => Some(Self::Active),
            "disabled" => Some(Self::Disabled),
            "deprecated" => Some(Self::Deprecated),
            "archived" => Some(Self::Archived),
            "retired" => Some(Self::Retired),
            _ => None,
        }
    }

    /// Allowed manifest values, in stable help/diagnostic order.
    pub fn allowed_values() -> &'static [&'static str] {
        &[
            "draft",
            "ready",
            "active",
            "disabled",
            "deprecated",
            "archived",
            "retired",
        ]
    }

    /// Stable manifest spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Ready => "ready",
            Self::Active => "active",
            Self::Disabled => "disabled",
            Self::Deprecated => "deprecated",
            Self::Archived => "archived",
            Self::Retired => "retired",
        }
    }
}

impl fmt::Display for ScenarioStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Stability tier for a scenario manifest.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub enum ScenarioStability {
    Provisional,
    Experimental,
    Unstable,
    Stable,
    Flaky,
    Deprecated,
}

impl ScenarioStability {
    /// Parses a lowercase manifest stability value.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "provisional" => Some(Self::Provisional),
            "experimental" => Some(Self::Experimental),
            "unstable" => Some(Self::Unstable),
            "stable" => Some(Self::Stable),
            "flaky" => Some(Self::Flaky),
            "deprecated" => Some(Self::Deprecated),
            _ => None,
        }
    }

    /// Allowed manifest values, in stable help/diagnostic order.
    pub fn allowed_values() -> &'static [&'static str] {
        &[
            "provisional",
            "experimental",
            "unstable",
            "stable",
            "flaky",
            "deprecated",
        ]
    }

    /// Stable manifest spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Provisional => "provisional",
            Self::Experimental => "experimental",
            Self::Unstable => "unstable",
            Self::Stable => "stable",
            Self::Flaky => "flaky",
            Self::Deprecated => "deprecated",
        }
    }
}

impl fmt::Display for ScenarioStability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Scenario intent. Manifests may keep the intent inline or point at a local file.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ScenarioIntent {
    /// Short inline summary/text, if present.
    pub summary: Option<String>,
    /// Local file containing the intent, if present.
    pub path: Option<String>,
}

impl ScenarioIntent {
    /// Stable one-line text for listing output.
    pub fn display_value(&self) -> String {
        self.summary
            .as_deref()
            .or(self.path.as_deref())
            .unwrap_or("")
            .to_string()
    }
}

/// Runtime topology declared by a scenario manifest.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct ScenarioTopology {
    /// Topology shape or boundary the scenario intends to validate.
    pub kind: Option<String>,
    /// Forge backend/provider named by the manifest.
    pub forge: Option<String>,
    /// Runner or CI harness named by the manifest.
    pub runner: Option<String>,
    /// Temper process/deployment shape named by the manifest.
    pub temper: Option<String>,
    /// Agent/model shape named by the manifest.
    pub agent_model: Option<String>,
}

impl ScenarioTopology {
    /// Returns true when the manifest did not declare any known topology facts.
    pub fn is_empty(&self) -> bool {
        self.kind.is_none()
            && self.forge.is_none()
            && self.runner.is_none()
            && self.temper.is_none()
            && self.agent_model.is_none()
    }

    /// Known topology facts in stable display order.
    pub fn field_values(&self) -> Vec<(&'static str, &str)> {
        [
            ("kind", self.kind.as_deref()),
            ("forge", self.forge.as_deref()),
            ("runner", self.runner.as_deref()),
            ("temper", self.temper.as_deref()),
            ("agent_model", self.agent_model.as_deref()),
        ]
        .into_iter()
        .filter_map(|(name, value)| value.map(|value| (name, value)))
        .collect()
    }
}

/// A local path reference discovered in a manifest.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct PathReference {
    /// Field path where the reference was found.
    pub field: String,
    /// Manifest value as written, after surrounding whitespace is trimmed.
    pub value: String,
    /// Path resolved relative to the scenario directory.
    pub resolved_path: PathBuf,
}

/// A Forge repository reference discovered in a manifest.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct RepositoryReference {
    /// Optional manifest-local repository id/alias.
    pub id: Option<String>,
    /// Provider repository in `owner/name` form.
    pub repo: String,
    /// Optional local checkout/fixture path from the same repository table.
    pub path: Option<String>,
}

/// A Forge issue reference discovered in a manifest.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct IssueReference {
    /// Repository reference. This may be an `owner/name` repo or a manifest-local
    /// repository id when the manifest declares repositories.
    pub repo: Option<String>,
    /// Positive issue number.
    pub number: u64,
}

/// Parsed and checked scenario manifest.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ScenarioManifest {
    pub name: String,
    pub status: ScenarioStatus,
    pub stability: ScenarioStability,
    pub intent: ScenarioIntent,
    pub topology: ScenarioTopology,
    pub assertion_templates: Vec<String>,
    pub repositories: Vec<RepositoryReference>,
    pub issues: Vec<IssueReference>,
    pub path_references: Vec<PathReference>,
}

/// One discovered scenario directory and its manifest file.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ScenarioEntry {
    pub scenario_path: PathBuf,
    pub manifest_path: PathBuf,
}

/// Report returned by [`crate::check_scenario`].
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CheckReport {
    pub scenario_path: PathBuf,
    pub manifest_path: Option<PathBuf>,
    pub manifest: Option<ScenarioManifest>,
    pub diagnostics: Vec<Diagnostic>,
}

impl CheckReport {
    /// Returns true when the report contains no error diagnostics.
    pub fn is_valid(&self) -> bool {
        !self
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }
}

/// Errors from loading a manifest through [`crate::load_manifest`].
#[derive(Debug, Error)]
pub enum ManifestLoadError {
    #[error("read {path}: {source}")]
    Read { path: PathBuf, source: io::Error },
    #[error("parse {path}: {source}")]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("invalid manifest {path}")]
    Invalid {
        path: PathBuf,
        diagnostics: Vec<Diagnostic>,
    },
}

/// Errors from scenario directory discovery.
#[derive(Debug, Error)]
pub enum DiscoverError {
    #[error("read {path}: {source}")]
    ReadDir { path: PathBuf, source: io::Error },
    #[error("{path} is not a directory")]
    NotDirectory { path: PathBuf },
}
