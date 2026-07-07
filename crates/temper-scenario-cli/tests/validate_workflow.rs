// SPDX-License-Identifier: MPL-2.0

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn temper_scenario(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_temper-scenario"))
        .args(args)
        .output()
        .expect("run temper-scenario")
}

fn write_inherited_basic_delivery_bundle(bundle: &Path, name: &str) {
    std::fs::create_dir_all(bundle).expect("create inherited bundle");
    std::fs::write(
        bundle.join("scenario.toml"),
        format!(
            "name = \"{name}\"\n\
             intent = \"Ephemeral validation bundle reusing checked-in basic-delivery fixtures.\"\n\
             [fixtures]\n\
             extends = \"scenarios/basic-delivery\"\n\
             [runner]\n\
             uses = \"basic-delivery\"\n\
             [expect]\n\
             template = \"single-pr-merged-source-closed\"\n\
             merged_pull_requests = 1\n\
             closed_parent_issues = 1\n\
             events = []\n\
             sequence = []\n\
             count = []\n\
             [[expect.checks]]\n\
             id = \"implementation-pr-landed\"\n\
             artifact = \"pull_request\"\n\
             state = \"merged\"\n\
             labels_cleared = [\"landing\"]\n\
             ci = \"passed\"\n\
             [[expect.checks]]\n\
             id = \"default-branch-updated\"\n\
             artifact = \"repo:service\"\n\
             branch = \"main\"\n\
             contains_engineer_diff = true\n"
        ),
    )
    .expect("write inherited manifest");
}

#[test]
fn validate_workflow_writes_evidence_markdown_and_json_artifacts() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scenario = dir.path().join("basic-delivery-validation");
    write_inherited_basic_delivery_bundle(&scenario, "basic-delivery-validation");
    let output_dir = dir.path().join("validation-artifacts");

    let output = temper_scenario(&[
        "validate",
        "--pr",
        "123",
        "--sha",
        "deadbeef",
        "--scenario",
        &scenario.to_string_lossy(),
        "--tier",
        "hermetic",
        "--output-dir",
        &output_dir.to_string_lossy(),
    ]);

    assert!(
        output.status.success(),
        "status: {:?}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("validation artifacts:"), "{stdout}");
    assert!(stdout.contains("run evidence:"), "{stdout}");
    assert!(stdout.contains("validation report:"), "{stdout}");
    assert!(stdout.contains("validation result:"), "{stdout}");

    let evidence_path = output_dir.join("run-evidence.json");
    let markdown_path = output_dir.join("validation-pr-123-deadbeef.md");
    let json_path = output_dir.join("validation-pr-123-deadbeef.json");
    assert!(evidence_path.is_file(), "evidence path: {evidence_path:?}");
    assert!(markdown_path.is_file(), "markdown path: {markdown_path:?}");
    assert!(json_path.is_file(), "json path: {json_path:?}");

    let evidence = read_json(&evidence_path);
    assert_eq!(evidence["scenario"]["tier"], "hermetic");
    assert_eq!(evidence["scenario"]["runner_id"], "basic-delivery");
    assert_eq!(evidence["assertions"]["status"], "passed");
    assert_eq!(evidence["assertions"]["unsupported"], 0);

    let markdown = std::fs::read_to_string(markdown_path).expect("read report");
    assert!(
        markdown.contains("Run evidence artifact ingested; scenario run was not rerun"),
        "{markdown}"
    );
    assert!(
        markdown.contains("Manifest assertion results were ingested"),
        "{markdown}"
    );
    assert!(
        markdown.contains("assertion passed `default-branch-updated`"),
        "{markdown}"
    );

    let result = read_json(&json_path);
    assert_eq!(result["schema"], "temper.validator.result.v1");
    assert_eq!(result["target"]["kind"], "implementation_pr");
    assert_eq!(result["target"]["repo"], "ai/temper");
    assert_eq!(result["target"]["ref"]["pr_number"], 123);
    assert_eq!(result["target"]["ref"]["merged_main_sha"], "deadbeef");
}

#[test]
fn validate_workflow_fails_clearly_for_missing_scenario() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output_dir = dir.path().join("validation-artifacts");
    let missing = dir.path().join("missing-scenario");

    let output = temper_scenario(&[
        "validate",
        "--pr",
        "123",
        "--sha",
        "deadbeef",
        "--scenario",
        &missing.to_string_lossy(),
        "--output-dir",
        &output_dir.to_string_lossy(),
    ]);

    assert!(!output.status.success(), "missing scenario should fail");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("temper-scenario validate: scenario check failed"),
        "{stderr}"
    );
    assert!(stderr.contains("scenario path does not exist"), "{stderr}");
    assert!(output_dir.is_dir(), "artifact directory should be retained");
}

#[test]
fn validate_workflow_live_with_missing_temper_bin_fails_clearly() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scenario = workspace_root().join("scenarios/basic-delivery");
    let missing_temper = dir.path().join("missing-temper");
    let output_dir = dir.path().join("validation-artifacts");

    let output = temper_scenario(&[
        "validate",
        "--pr",
        "123",
        "--sha",
        "deadbeef",
        "--scenario",
        &scenario.to_string_lossy(),
        "--tier",
        "live",
        "--temper-bin",
        &missing_temper.to_string_lossy(),
        "--output-dir",
        &output_dir.to_string_lossy(),
    ]);

    assert!(
        !output.status.success(),
        "missing live temper binary should fail"
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("live basic-delivery --temper-bin path does not exist"),
        "{stderr}"
    );
    assert!(stderr.contains("missing-temper"), "{stderr}");
    assert!(output_dir.is_dir(), "artifact directory should be retained");
}

#[test]
fn validate_workflow_rejects_unknown_tier_before_running() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scenario = workspace_root().join("scenarios/basic-delivery");

    let output = temper_scenario(&[
        "validate",
        "--pr",
        "123",
        "--sha",
        "deadbeef",
        "--scenario",
        &scenario.to_string_lossy(),
        "--output-dir",
        &dir.path().join("validation-artifacts").to_string_lossy(),
        "--tier",
        "medium",
    ]);

    assert!(!output.status.success(), "unknown tier should fail");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("unknown --tier `medium`"), "{stderr}");
    assert!(stderr.contains("expected hermetic or live"), "{stderr}");
}

fn read_json(path: &Path) -> serde_json::Value {
    let source = std::fs::read_to_string(path).expect("read json");
    serde_json::from_str(&source).expect("parse json")
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate directory has workspace crates parent")
        .parent()
        .expect("crates directory has workspace root parent")
        .to_path_buf()
}
