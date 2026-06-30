// SPDX-License-Identifier: MPL-2.0

//! Scenario manifest parsing, discovery, and validation.
//!
//! The first scenario tooling deliberately keeps the manifest model small and
//! tolerant: the required metadata fields are checked strictly, while optional
//! repository, issue, and local path references are discovered from the common
//! first-pass shapes (`repos`/`repositories`, `issues`, `paths`/`files`, and
//! `intent.path`). That lets early scenario fixtures evolve without hiding the
//! problems that matter to CI: unknown enum values, broken local references, and
//! malformed Forge references.

mod assertion_templates;
mod diagnostics;
mod discovery;
mod issue_refs;
mod manifest;
mod parse;
mod path_refs;
mod repo_refs;
mod toml_helpers;
mod validation_report;

pub use assertion_templates::{
    ASSERTION_TEMPLATE_CATALOG, ASSERTION_TEMPLATE_NAMES, AssertionTemplate,
    is_known_assertion_template,
};
pub use diagnostics::{Diagnostic, Severity};
pub use discovery::{check_scenario, check_scenarios, discover_scenarios, resolve_manifest_path};
pub use manifest::{
    CheckReport, DiscoverError, IssueReference, ManifestLoadError, PathReference,
    RepositoryReference, ScenarioEntry, ScenarioIntent, ScenarioManifest, ScenarioStability,
    ScenarioStatus,
};
pub use parse::{load_manifest, parse_manifest_str};
pub use validation_report::{
    AcceptanceCriterion, EvidenceEntry, EvidenceKind, FollowUpIssueIntent, ValidatedClaim,
    ValidationReport, ValidationStatus, ValidationTarget, ValidationVerdict,
};

/// Default directory scanned by the CLI when no scenario root is supplied.
pub const DEFAULT_SCENARIOS_DIR: &str = "scenarios";

/// Manifest filenames recognized inside a scenario directory, in priority order.
pub const MANIFEST_FILE_NAMES: &[&str] =
    &["scenario.toml", "manifest.toml", "temper-scenario.toml"];

#[cfg(test)]
mod tests;
