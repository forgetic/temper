// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::path::PathBuf;

use crate::{
    ASSERTION_TEMPLATE_CATALOG, ASSERTION_TEMPLATE_NAMES, AcceptanceCriterion, Diagnostic,
    EvidenceEntry, EvidenceKind, FollowUpIssueIntent, ScenarioStability, ScenarioStatus,
    ValidatedClaim, ValidationReport, ValidationStatus, ValidationVerdict, check_scenario,
    discover_scenarios, load_manifest, load_resolved_manifest_toml, parse_manifest_str,
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
fn assertion_template_catalog_names_are_stable() {
    let catalog_names = ASSERTION_TEMPLATE_CATALOG
        .iter()
        .map(|template| template.name)
        .collect::<Vec<_>>();

    assert_eq!(catalog_names.as_slice(), ASSERTION_TEMPLATE_NAMES);
}

#[test]
fn renders_validation_report_markdown_sections() {
    let mut report = ValidationReport::new(123, "deadbeef", ValidationVerdict::Inconclusive);
    report.validated_claims.push(
        ValidatedClaim::new(
            "Scenario manifest validates for the supplied path.",
            ValidationStatus::Observed,
        )
        .with_evidence("scenario check evidence"),
    );
    report.acceptance_criteria.push(
        AcceptanceCriterion::new(
            "The report records observable acceptance criteria.",
            ValidationStatus::Satisfied,
        )
        .with_evidence("criterion evidence"),
    );
    report.evidence.push(
        EvidenceEntry::new(EvidenceKind::ScenarioCheck, "Scenario check completed.")
            .with_detail("checked scenarios/basic-delivery"),
    );
    report
        .limitations
        .push("Temporary harness does not query live Forgejo state.".to_string());
    report.follow_up = Some(
        FollowUpIssueIntent::new(
            "Implement workflow-native validation",
            "Replace the manual validate-pr bridge.",
        )
        .with_label("validation"),
    );

    let markdown = report.render_markdown();

    for section in [
        "## Verdict",
        "## Validated claims",
        "## Acceptance criteria",
        "## Evidence",
        "## Limitations",
        "## Follow-up intent",
    ] {
        assert!(markdown.contains(section), "missing {section}:\n{markdown}");
    }
    assert!(markdown.contains("PR: #123"), "{markdown}");
    assert!(
        markdown.contains("Merged/main SHA: `deadbeef`"),
        "{markdown}"
    );
    assert!(markdown.contains("Verdict: inconclusive"), "{markdown}");
    assert!(markdown.contains("**scenario check**"), "{markdown}");
    assert!(
        markdown.contains("Temporary harness does not query live Forgejo state."),
        "{markdown}"
    );
    assert!(
        markdown.contains("Implement workflow-native validation"),
        "{markdown}"
    );
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
    assert!(manifest.runner.uses.is_none());
    assert!(manifest.topology.is_empty());
    assert!(manifest.assertion_templates.is_empty());
    assert_eq!(manifest.repositories.len(), 1);
    assert_eq!(manifest.repositories[0].repo, "ai/temper");
    assert_eq!(manifest.issues.len(), 1);
    assert_eq!(manifest.issues[0].number, 37);
    assert_eq!(manifest.path_references.len(), 2);
}

#[test]
fn parses_manifest_runner_selector() {
    let manifest = parse_manifest_str(
        r##"
name = "renamed-delivery"
status = "ready"
stability = "experimental"
intent = "Runner metadata should select a reusable runner independently from name."

[runner]
uses = "basic-delivery"
"##,
        ".",
    )
    .expect("runner selector manifest is valid");

    assert_eq!(manifest.name, "renamed-delivery");
    assert_eq!(manifest.runner.uses.as_deref(), Some("basic-delivery"));
}

#[test]
fn rejects_malformed_runner_selector() {
    let diagnostics = parse_manifest_str(
        r##"
name = "broken-runner"
status = "ready"
stability = "experimental"
intent = "Bad runner metadata should fail."

[runner]
uses = 7
"##,
        ".",
    )
    .expect_err("non-string runner selector is invalid");

    assert_has_field(&diagnostics, "runner.uses");
    assert_has_message(&diagnostics, "must be a string");
}

#[test]
fn parses_manifest_topology_facts() {
    let manifest = parse_manifest_str(
        r##"
name = "topology-case"
status = "ready"
stability = "experimental"
intent = "Topology metadata should be exposed to runners and reports."

[topology]
kind = "single-repo-forgejo-standalone"
forge = "forgejo"
runner = "forgejo-actions-host"
temper = "standalone"
agent_model = "scripted-fake-llm"
"##,
        ".",
    )
    .expect("topology manifest is valid");

    assert_eq!(
        manifest.topology.field_values(),
        vec![
            ("kind", "single-repo-forgejo-standalone"),
            ("forge", "forgejo"),
            ("runner", "forgejo-actions-host"),
            ("temper", "standalone"),
            ("agent_model", "scripted-fake-llm"),
        ]
    );
}

#[test]
fn rejects_malformed_topology_facts() {
    let diagnostics = parse_manifest_str(
        r##"
name = "broken-topology"
status = "ready"
stability = "experimental"
intent = "Bad topology metadata should fail."

[topology]
kind = 7
"##,
        ".",
    )
    .expect_err("non-string topology facts are invalid");

    assert_has_field(&diagnostics, "topology.kind");
    assert_has_message(&diagnostics, "must be a string");
}

#[test]
fn parses_known_assertion_templates_as_manifest_metadata() {
    let manifest = parse_manifest_str(
        r##"
name = "templated"
status = "ready"
stability = "experimental"
intent = "Template metadata should validate independently from explicit checks."

[expect]
template = "single-pr-merged-source-closed"
templates = ["no-duplicate-prs", "quiescent-after-merge"]

[[expect.checks]]
id = "explicit-check-remains-supported"
state = "merged"
"##,
        ".",
    )
    .expect("known templates are valid");

    assert_eq!(
        manifest.assertion_templates,
        vec![
            "single-pr-merged-source-closed".to_string(),
            "no-duplicate-prs".to_string(),
            "quiescent-after-merge".to_string()
        ]
    );
}

#[test]
fn rejects_unknown_assertion_templates() {
    let diagnostics = parse_manifest_str(
        r##"
name = "templated"
status = "ready"
stability = "experimental"
intent = "Unknown templates should fail manifest validation."

[expect]
templates = ["single-pr-merged-source-closed", "surprise-contract"]
"##,
        ".",
    )
    .expect_err("unknown template is invalid");

    assert_has_message(
        &diagnostics,
        "unknown assertion template `surprise-contract`",
    );
    assert_has_field(&diagnostics, "expect.templates[1]");
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
    assert_eq!(
        manifest.assertion_templates,
        vec!["single-pr-merged-source-closed".to_string()]
    );

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
fn fixture_inheritance_supplies_defaults_and_keeps_path_origins() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = dir.path().join("renamed-delivery");
    fs::create_dir(&bundle).expect("bundle dir");
    fs::write(
        bundle.join("scenario.toml"),
        r##"
name = "renamed-delivery"
intent = "Reuse the checked-in basic-delivery fixture material without copying it."

[fixtures]
extends = "scenarios/basic-delivery"

[runner]
uses = "basic-delivery"
"##,
    )
    .expect("write manifest");

    let report = check_scenario(&bundle);

    assert!(report.is_valid(), "diagnostics: {:#?}", report.diagnostics);
    let manifest = report.manifest.expect("resolved manifest");
    assert_eq!(manifest.name, "renamed-delivery");
    assert_eq!(manifest.runner.uses.as_deref(), Some("basic-delivery"));
    assert_eq!(manifest.status, ScenarioStatus::Active);
    assert_eq!(manifest.stability, ScenarioStability::Provisional);
    assert_eq!(
        manifest.topology.kind.as_deref(),
        Some("single-repo-forgejo-standalone")
    );

    let workflow = manifest
        .path_references
        .iter()
        .find(|reference| reference.field == "workflow.path")
        .expect("inherited workflow path reference");
    assert_eq!(workflow.value, "config/workflow.json");
    assert_eq!(
        workflow.resolved_path,
        workspace_root().join("scenarios/basic-delivery/config/workflow.json")
    );

    let resolved_toml =
        load_resolved_manifest_toml(bundle.join("scenario.toml")).expect("resolved manifest TOML");
    let workflow_path = resolved_toml
        .get("workflow")
        .and_then(toml::Value::as_table)
        .and_then(|workflow| workflow.get("path"))
        .and_then(toml::Value::as_str)
        .expect("resolved workflow path");
    assert_eq!(
        PathBuf::from(workflow_path),
        workspace_root().join("scenarios/basic-delivery/config/workflow.json")
    );
    assert!(
        !bundle.join("config/workflow.json").exists(),
        "the child bundle should not need to copy fixture files"
    );
}

#[test]
fn rejects_missing_fixture_inheritance_base() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(
        dir.path().join("scenario.toml"),
        r##"
name = "missing-base"
status = "ready"
stability = "experimental"
intent = "Missing base should be diagnosed."

[fixtures]
extends = "scenarios/does-not-exist"
"##,
    )
    .expect("write manifest");

    let report = check_scenario(dir.path());

    assert!(!report.is_valid());
    assert_has_field(&report.diagnostics, "fixtures.extends");
    assert_has_message(
        &report.diagnostics,
        "fixture inheritance base does not exist",
    );
}

#[test]
fn rejects_cyclic_and_unsafe_fixture_inheritance() {
    let cycle = tempfile::tempdir().expect("tempdir");
    fs::write(
        cycle.path().join("scenario.toml"),
        r##"
name = "self-cycle"
status = "ready"
stability = "experimental"
intent = "Self inheritance should be diagnosed."

[fixtures]
extends = "."
"##,
    )
    .expect("write cycle manifest");

    let report = check_scenario(cycle.path());
    assert!(!report.is_valid());
    assert_has_field(&report.diagnostics, "fixtures.extends");
    assert_has_message(&report.diagnostics, "fixture inheritance cycle");

    let unsafe_bundle = tempfile::tempdir().expect("tempdir");
    fs::write(
        unsafe_bundle.path().join("scenario.toml"),
        r##"
name = "unsafe-base"
status = "ready"
stability = "experimental"
intent = "Parent-directory escapes should be diagnosed."

[fixtures]
extends = "../outside"
"##,
    )
    .expect("write unsafe manifest");

    let report = check_scenario(unsafe_bundle.path());
    assert!(!report.is_valid());
    assert_has_field(&report.diagnostics, "fixtures.extends");
    assert_has_message(&report.diagnostics, "without `..` components");
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

fn assert_has_field(diagnostics: &[Diagnostic], field: &str) {
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.field.as_deref() == Some(field)),
        "diagnostics did not contain field {field:?}: {diagnostics:#?}"
    );
}
