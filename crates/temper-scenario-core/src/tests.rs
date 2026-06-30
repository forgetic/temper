// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::path::PathBuf;

use crate::{
    Diagnostic, ScenarioStability, ScenarioStatus, check_scenario, discover_scenarios,
    load_manifest, parse_manifest_str,
};

fn valid_manifest() -> &'static str {
    r##"
schema_version = 1
name = "basic-delivery"
status = "ready"
stability = "experimental"
intent = "Exercise a simple issue-to-PR delivery path."
files = ["README.md"]

[[repositories]]
id = "primary"
repo = "ai/temper"
path = "repo"

[[issues]]
repo = "primary"
number = 37
"##
}

#[test]
fn parses_valid_manifest_and_references() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("README.md"), "intent").expect("write readme");
    fs::create_dir(dir.path().join("repo")).expect("repo dir");

    let manifest = parse_manifest_str(valid_manifest(), dir.path()).expect("valid manifest");

    assert_eq!(manifest.name, "basic-delivery");
    assert_eq!(manifest.status, ScenarioStatus::Ready);
    assert_eq!(manifest.stability, ScenarioStability::Experimental);
    assert_eq!(manifest.repositories.len(), 1);
    assert_eq!(manifest.repositories[0].repo, "ai/temper");
    assert_eq!(manifest.issues.len(), 1);
    assert_eq!(manifest.issues[0].number, 37);
    assert_eq!(manifest.path_references.len(), 2);
}

#[test]
fn supports_metadata_in_scenario_table_and_intent_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("intent.md"), "intent").expect("write intent");
    let manifest = parse_manifest_str(
        r##"
[scenario]
name = "table-shape"
status = "draft"
stability = "unstable"

[scenario.intent]
path = "intent.md"
"##,
        dir.path(),
    )
    .expect("valid manifest");

    assert_eq!(manifest.name, "table-shape");
    assert_eq!(manifest.intent.path.as_deref(), Some("intent.md"));
    assert_eq!(manifest.path_references[0].field, "scenario.intent.path");
}

#[test]
fn file_tables_validate_only_nested_path_fields() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("README.md"), "intent").expect("write readme");

    let manifest = parse_manifest_str(
        r##"
name = "file-table"
status = "ready"
stability = "experimental"
intent = "file metadata should not be treated as a path"
files = [{ path = "README.md", role = "intent" }]
"##,
        dir.path(),
    )
    .expect("valid manifest");

    assert_eq!(manifest.path_references.len(), 1);
    assert_eq!(manifest.path_references[0].field, "files[0].path");
}

#[test]
fn reports_required_fields_and_known_enum_values() {
    let diagnostics = parse_manifest_str(
        r##"
name = "broken"
status = "shipped"
stability = "forever"
intent = "nope"
"##,
        ".",
    )
    .expect_err("invalid manifest");

    assert_has_message(&diagnostics, "unknown status");
    assert_has_message(&diagnostics, "unknown stability");
}

#[test]
fn validates_local_path_references_and_missing_files() {
    let diagnostics = parse_manifest_str(
        r##"
name = "broken-paths"
status = "ready"
stability = "experimental"
intent = { path = "missing.md" }
files = ["../outside.txt", "https://example.invalid/file"]
"##,
        ".",
    )
    .expect_err("invalid manifest");

    assert_has_message(&diagnostics, "referenced path does not exist");
    assert_has_message(&diagnostics, "without `..` components");
    assert_has_message(&diagnostics, "not a URL");
}

#[test]
fn validates_repository_and_issue_references() {
    let diagnostics = parse_manifest_str(
        r##"
name = "broken-refs"
status = "ready"
stability = "experimental"
intent = "bad references"

[[repositories]]
id = "primary"
repo = "not-a-repo"

[[issues]]
repo = "missing-alias"
number = 0
"##,
        ".",
    )
    .expect_err("invalid manifest");

    assert_has_message(&diagnostics, "owner/name");
    assert_has_message(&diagnostics, "issue number must be positive");
}

#[test]
fn load_manifest_checks_filesystem_references() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("README.md"), "intent").expect("write readme");
    fs::create_dir(dir.path().join("repo")).expect("repo dir");
    let path = dir.path().join("scenario.toml");
    fs::write(&path, valid_manifest()).expect("write manifest");

    let manifest = load_manifest(&path).expect("valid manifest");

    assert_eq!(manifest.name, "basic-delivery");
}

#[test]
fn load_manifest_accepts_checked_in_basic_delivery_shape() {
    let manifest_path = workspace_root().join("scenarios/basic-delivery/scenario.toml");
    let manifest = load_manifest(&manifest_path).expect("checked-in scenario manifest is valid");

    assert_eq!(manifest.name, "basic-delivery");
    assert_eq!(manifest.status, ScenarioStatus::Active);
    assert_eq!(manifest.stability, ScenarioStability::Provisional);
    assert_eq!(manifest.repositories.len(), 1);
    assert_eq!(manifest.repositories[0].id.as_deref(), Some("service"));
    assert_eq!(manifest.repositories[0].repo, "acme/service");

    let path_fields = manifest
        .path_references
        .iter()
        .map(|reference| reference.field.as_str())
        .collect::<Vec<_>>();
    for field in [
        "workflow.path",
        "repos[0].seed_path",
        "repos[0].ci_source",
        "repos[0].ci_seed_path",
        "issues[0].body",
    ] {
        assert!(
            path_fields.contains(&field),
            "expected local path reference {field:?} in {path_fields:#?}"
        );
    }
}

#[test]
fn check_scenario_reports_missing_manifest() {
    let dir = tempfile::tempdir().expect("tempdir");
    let report = check_scenario(dir.path());

    assert!(!report.is_valid());
    assert_has_message(&report.diagnostics, "no scenario manifest found");
}

#[test]
fn discovers_scenario_directories_in_stable_order() {
    let root = tempfile::tempdir().expect("tempdir");
    for name in ["zeta", "alpha"] {
        let scenario = root.path().join(name);
        fs::create_dir(&scenario).expect("scenario dir");
        fs::write(scenario.join("scenario.toml"), valid_manifest()).expect("manifest");
    }

    let entries = discover_scenarios(root.path()).expect("discover scenarios");
    let names = entries
        .iter()
        .map(|entry| {
            entry
                .scenario_path
                .file_name()
                .expect("name")
                .to_string_lossy()
                .to_string()
        })
        .collect::<Vec<_>>();

    assert_eq!(names, vec!["alpha", "zeta"]);
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate directory has workspace crates parent")
        .parent()
        .expect("crates directory has workspace root parent")
        .to_path_buf()
}

fn assert_has_message(diagnostics: &[Diagnostic], needle: &str) {
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains(needle)),
        "diagnostics did not contain {needle:?}: {diagnostics:#?}"
    );
}
