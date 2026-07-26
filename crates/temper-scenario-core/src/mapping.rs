// SPDX-License-Identifier: MPL-2.0

use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use crate::{ScenarioManifest, ScenarioMetadataContext, ScenarioStatus};

#[path = "mapping/resolver.rs"]
mod resolver;

pub use resolver::resolve_feature_scenario;

/// Stable schema id for CI-consumable feature-to-scenario resolution output.
pub const FEATURE_SCENARIO_MAPPING_SCHEMA: &str = "temper.scenario.feature-mapping.v1";

/// A typed Forge issue identity used by feature-scenario mappings.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ForgeIssueKey {
    pub repo: String,
    pub number: u64,
}

impl ForgeIssueKey {
    pub fn new(repo: impl Into<String>, number: u64) -> Result<Self, String> {
        let value = Self {
            repo: repo.into(),
            number,
        };
        validate_repo(&value.repo)?;
        if value.number == 0 {
            return Err("issue number must be positive".to_string());
        }
        Ok(value)
    }
}

impl FromStr for ForgeIssueKey {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value.trim();
        let (repo, number) = value.rsplit_once('#').ok_or_else(|| {
            "must be in `owner/name#number` form (for example `ai/temper#778`)".to_string()
        })?;
        let number = number
            .parse::<u64>()
            .map_err(|_| "issue number must be a positive integer".to_string())?;
        Self::new(repo, number)
    }
}

impl fmt::Display for ForgeIssueKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}#{}", self.repo, self.number)
    }
}

impl Serialize for ForgeIssueKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ForgeIssueKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// Explicit author intent for comparison with the supplied landing base.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeatureMappingChange {
    New,
    Updated,
}

impl FeatureMappingChange {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "new" => Some(Self::New),
            "updated" => Some(Self::Updated),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Updated => "updated",
        }
    }
}

impl fmt::Display for FeatureMappingChange {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Typed `[validation]` metadata owned by one scenario manifest.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct FeatureScenarioMapping {
    pub feature: ForgeIssueKey,
    pub plan: Option<ForgeIssueKey>,
    pub source_branch: String,
    pub change: FeatureMappingChange,
}

impl FeatureScenarioMapping {
    pub fn identity(&self, scenario_name: &str) -> String {
        format!("{}:{scenario_name}", self.feature)
    }
}

/// Typed claim → stimulus → observable → assertion authoring contract.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ScenarioFeatureContract {
    pub claim: String,
    pub stimulus: String,
    pub observable: String,
    pub assertion: String,
    pub runtime_budget_seconds: u64,
    pub jig_script_path: String,
}

/// Landing-base classification proved by the resolver.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeatureScenarioBaseComparison {
    New,
    Updated,
}

/// Deterministic mapping result suitable for CLI JSON, CI artifacts, validator
/// context, and Forge audit rendering.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResolvedFeatureScenario {
    pub schema: String,
    pub mapping_id: String,
    pub feature: ForgeIssueKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<ForgeIssueKey>,
    pub scenario_name: String,
    pub scenario_path: String,
    pub manifest_path: String,
    pub source_branch: String,
    pub head_sha: String,
    pub landing_base: String,
    pub landing_base_sha: String,
    pub base_comparison: FeatureScenarioBaseComparison,
    pub content_changed_from_base: bool,
    pub change_intent: FeatureMappingChange,
    pub digest: String,
}

impl ResolvedFeatureScenario {
    /// Project the resolved identity into the workflow-native validator context.
    pub fn validator_context(&self, manifest: &ScenarioManifest) -> ScenarioMetadataContext {
        ScenarioMetadataContext {
            name: self.scenario_name.clone(),
            path: self.scenario_path.clone(),
            status: manifest.status.to_string(),
            stability: manifest.stability.to_string(),
            templates: manifest.assertion_templates.clone(),
            commit: Some(self.head_sha.clone()),
            mapping_id: Some(self.mapping_id.clone()),
            feature: Some(self.feature.to_string()),
            plan: self.plan.as_ref().map(ToString::to_string),
            source_branch: Some(self.source_branch.clone()),
            digest: Some(self.digest.clone()),
        }
    }
}

/// Inputs for deterministic feature-to-scenario resolution.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ResolveFeatureScenarioRequest {
    pub checkout_root: PathBuf,
    pub scenarios_root: PathBuf,
    pub feature: ForgeIssueKey,
    pub landing_base: String,
    pub expected_digest: Option<String>,
}

impl ResolveFeatureScenarioRequest {
    pub fn new(
        checkout_root: impl Into<PathBuf>,
        scenarios_root: impl Into<PathBuf>,
        feature: ForgeIssueKey,
        landing_base: impl Into<String>,
    ) -> Self {
        Self {
            checkout_root: checkout_root.into(),
            scenarios_root: scenarios_root.into(),
            feature,
            landing_base: landing_base.into(),
            expected_digest: None,
        }
    }
}

#[derive(Debug, Error)]
pub enum FeatureScenarioResolveError {
    #[error("invalid feature-scenario resolver input: {0}")]
    InvalidInput(String),
    #[error("failed to inspect git checkout: {0}")]
    Git(String),
    #[error("scenario corpus contains an invalid manifest at {path}: {diagnostics}")]
    InvalidManifest { path: String, diagnostics: String },
    #[error("no scenario explicitly maps feature `{feature}` under {root}")]
    Missing {
        feature: ForgeIssueKey,
        root: String,
    },
    #[error("feature `{feature}` maps to multiple scenarios: {paths}")]
    Ambiguous {
        feature: ForgeIssueKey,
        paths: String,
    },
    #[error("mapped scenario `{path}` is not active (status is `{status}`)")]
    Inactive {
        path: String,
        status: ScenarioStatus,
    },
    #[error("unsafe mapped scenario `{path}`: {reason}")]
    Unsafe { path: String, reason: String },
    #[error(
        "mapped scenario `{path}` has uncommitted or untracked content; resolution must describe the checked-out HEAD exactly"
    )]
    Dirty { path: String },
    #[error(
        "mapped scenario `{path}` is unchanged from landing base `{base}`; a feature scenario must be new or deliberately updated"
    )]
    Unchanged { path: String, base: String },
    #[error(
        "mapped scenario `{path}` is new relative to `{base}` but validation.change is `{actual}`; use `new`"
    )]
    NewIntent {
        path: String,
        base: String,
        actual: FeatureMappingChange,
    },
    #[error(
        "mapped scenario `{path}` already exists, or mapped this feature, at landing base `{base}` but validation.change is `{actual}`; use `updated` to record deliberate update intent"
    )]
    UpdatedIntent {
        path: String,
        base: String,
        actual: FeatureMappingChange,
    },
    #[error("failed to digest mapped scenario: {0}")]
    Digest(String),
    #[error("mapped scenario digest mismatch: expected `{expected}`, computed `{actual}`")]
    DigestMismatch { expected: String, actual: String },
}

/// Validate a source branch without accepting ref escapes or option-like names.
pub fn validate_source_branch(value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err("must not be empty".to_string());
    }
    if value.starts_with('-')
        || value.starts_with('/')
        || value.ends_with('/')
        || value.ends_with('.')
        || value == "@"
        || value.contains("..")
        || value.contains("@{")
        || value.split('/').any(|component| {
            component.is_empty() || component == "." || component.ends_with(".lock")
        })
        || value.chars().any(|character| {
            character.is_control()
                || character.is_whitespace()
                || matches!(character, '~' | '^' | ':' | '?' | '*' | '[' | '\\')
        })
    {
        return Err("must be a safe Git branch name without whitespace, ref metacharacters, or traversal components".to_string());
    }
    Ok(())
}

fn validate_repo(repo: &str) -> Result<(), String> {
    let mut parts = repo.split('/');
    let owner = parts.next().unwrap_or_default();
    let name = parts.next().unwrap_or_default();
    if owner.is_empty()
        || name.is_empty()
        || matches!(owner, "." | "..")
        || matches!(name, "." | "..")
        || parts.next().is_some()
        || !owner.chars().all(repo_character)
        || !name.chars().all(repo_character)
    {
        return Err("repository must be in safe `owner/name` form".to_string());
    }
    Ok(())
}

fn repo_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
}
