// SPDX-License-Identifier: MPL-2.0

use std::fs;

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

fn assert_has_message(diagnostics: &[Diagnostic], needle: &str) {
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains(needle)),
        "diagnostics did not contain {needle:?}: {diagnostics:#?}"
    );
}
