// SPDX-License-Identifier: MPL-2.0

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn temper_scenario(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_temper-scenario"))
        .args(args)
        .output()
        .expect("run temper-scenario")
}

fn copy_dir_all(source: &Path, target: &Path) {
    std::fs::create_dir_all(target).expect("create target directory");
    for entry in std::fs::read_dir(source).expect("read source directory") {
        let entry = entry.expect("read source entry");
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if source_path.is_dir() {
            copy_dir_all(&source_path, &target_path);
        } else {
            std::fs::copy(&source_path, &target_path).expect("copy file");
        }
    }
}

fn select_runner_for_bundle(bundle: &Path, name: &str, runner: &str) {
    let manifest_path = bundle.join("scenario.toml");
    let manifest = std::fs::read_to_string(&manifest_path).expect("read manifest");
    let manifest = manifest.replacen(
        "name = \"basic-delivery\"",
        &format!("name = \"{name}\""),
        1,
    );
    let manifest = manifest.replacen(
        "[topology]\n",
        &format!("[runner]\nuses = \"{runner}\"\n\n[topology]\n"),
        1,
    );
    std::fs::write(&manifest_path, manifest).expect("write manifest");
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate directory has workspace crates parent")
        .parent()
        .expect("crates directory has workspace root parent")
        .to_path_buf()
}

#[test]
fn run_succeeds_for_checked_in_basic_delivery_scenario() {
    let scenario = workspace_root().join("scenarios/basic-delivery");

    let output = temper_scenario(&["run", &scenario.to_string_lossy()]);

    assert!(
        output.status.success(),
        "status: {:?}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("scenario: basic-delivery"), "{stdout}");
    assert!(stdout.contains("source: checked-in scenario"), "{stdout}");
    assert!(stdout.contains("confidence tier: hermetic"), "{stdout}");
    assert!(stdout.contains("not a live Forgejo proof"), "{stdout}");
    assert!(stdout.contains("manifest topology:"), "{stdout}");
    assert!(
        stdout.contains("kind: single-repo-forgejo-standalone"),
        "{stdout}"
    );
    assert!(stdout.contains("forge: forgejo"), "{stdout}");
    assert!(stdout.contains("runner: forgejo-actions-host"), "{stdout}");
    assert!(stdout.contains("temper: standalone"), "{stdout}");
    assert!(
        stdout.contains("agent_model: scripted-fake-llm"),
        "{stdout}"
    );
    assert!(stdout.contains("verdict: passed"), "{stdout}");
    assert!(stdout.contains("seeded issue: #"), "{stdout}");
    assert!(stdout.contains("closed as code"), "{stdout}");
    assert!(stdout.contains("implementation PR: #"), "{stdout}");
    assert!(stdout.contains("merged with passing CI"), "{stdout}");
    assert!(stdout.contains("closed parent issues: 1"), "{stdout}");
    assert!(!stdout.contains("open (not merged)"), "{stdout}");
    assert!(stdout.contains("actions:"), "{stdout}");
}

#[test]
fn run_succeeds_for_ephemeral_basic_delivery_bundle() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = dir.path().join("basic-delivery-copy");
    copy_dir_all(&workspace_root().join("scenarios/basic-delivery"), &bundle);

    let check = temper_scenario(&["check", &bundle.to_string_lossy()]);
    assert!(
        check.status.success(),
        "status: {:?}\nstdout: {}\nstderr: {}",
        check.status,
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr)
    );

    let output = temper_scenario(&["run", "--tier", "hermetic", &bundle.to_string_lossy()]);

    assert!(
        output.status.success(),
        "status: {:?}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("scenario: basic-delivery"), "{stdout}");
    assert!(
        stdout.contains("source: ephemeral validation bundle"),
        "{stdout}"
    );
    assert!(stdout.contains("confidence tier: hermetic"), "{stdout}");
    assert!(
        stdout.contains("kind: single-repo-forgejo-standalone"),
        "{stdout}"
    );
    assert!(stdout.contains("verdict: passed"), "{stdout}");
}

#[test]
fn run_uses_runner_selector_for_different_manifest_name() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = dir.path().join("renamed-delivery");
    copy_dir_all(&workspace_root().join("scenarios/basic-delivery"), &bundle);
    select_runner_for_bundle(&bundle, "renamed-delivery", "basic-delivery");

    let check = temper_scenario(&["check", &bundle.to_string_lossy()]);
    assert!(
        check.status.success(),
        "status: {:?}\nstdout: {}\nstderr: {}",
        check.status,
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr)
    );

    let output = temper_scenario(&["run", "--tier", "hermetic", &bundle.to_string_lossy()]);

    assert!(
        output.status.success(),
        "status: {:?}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("scenario: basic-delivery"), "{stdout}");
    assert!(
        stdout.contains("source: ephemeral validation bundle"),
        "{stdout}"
    );
    assert!(stdout.contains("verdict: passed"), "{stdout}");
}

#[test]
fn validate_pr_uses_runner_selector_for_different_manifest_name() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = dir.path().join("renamed-delivery");
    copy_dir_all(&workspace_root().join("scenarios/basic-delivery"), &bundle);
    select_runner_for_bundle(&bundle, "renamed-delivery", "basic-delivery");
    let output_dir = dir.path().join("reports");

    let output = temper_scenario(&[
        "validate-pr",
        "--pr",
        "123",
        "--sha",
        "deadbeef",
        "--scenario",
        &bundle.to_string_lossy(),
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
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let markdown = std::fs::read_to_string(PathBuf::from(stdout.trim())).expect("read report");
    assert!(
        markdown.contains("scenario: `renamed-delivery`"),
        "{markdown}"
    );
    assert!(
        markdown.contains("runner: `basic-delivery` selected by runner.uses"),
        "{markdown}"
    );
    assert!(
        markdown.contains("Deterministic basic-delivery scenario run completed successfully"),
        "{markdown}"
    );
}

#[test]
fn run_live_tier_with_missing_temper_binary_fails_before_substituting_hermetic_runner() {
    let scenario = workspace_root().join("scenarios/basic-delivery");
    let missing_temper = tempfile::tempdir()
        .expect("tempdir")
        .path()
        .join("missing-temper");

    let output = temper_scenario(&[
        "run",
        "--tier",
        "live",
        "--temper-bin",
        &missing_temper.to_string_lossy(),
        &scenario.to_string_lossy(),
    ]);

    assert!(
        !output.status.success(),
        "missing live temper binary should fail"
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("--temper-bin path does not exist"),
        "{stderr}"
    );
    assert!(stderr.contains("missing-temper"), "{stderr}");
    assert!(!stderr.contains("seeded issue:"), "{stderr}");
}

#[test]
fn run_help_documents_tier_selector() {
    let output = temper_scenario(&["run", "--help"]);

    assert!(output.status.success(), "status: {:?}", output.status);
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(
        stdout.contains("Usage: temper-scenario run [--tier <hermetic|live>] [--temper-bin <PATH>] <SCENARIO_PATH>"),
        "{stdout}"
    );
    assert!(
        stdout.contains("The live tier for `basic-delivery` boots the shared"),
        "{stdout}"
    );
    assert!(stdout.contains("TEMPER_SCENARIO_TEMPER_BIN"), "{stdout}");
}

#[test]
fn run_rejects_unknown_tier() {
    let scenario = workspace_root().join("scenarios/basic-delivery");

    let output = temper_scenario(&["run", "--tier", "medium", &scenario.to_string_lossy()]);

    assert!(!output.status.success(), "unknown tier should fail");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("unknown --tier `medium`"), "{stderr}");
    assert!(stderr.contains("expected hermetic or live"), "{stderr}");
}

#[test]
fn run_rejects_unsupported_tier_from_registry() {
    let scenario = workspace_root().join("scenarios/implementation-pr-handoff");

    let output = temper_scenario(&["run", "--tier", "live", &scenario.to_string_lossy()]);

    assert!(!output.status.success(), "unsupported tier should fail");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("unsupported tier `live` for runner `implementation-pr-handoff`"),
        "{stderr}"
    );
    assert!(stderr.contains("supported tiers: hermetic"), "{stderr}");
    assert!(stderr.contains("supported runner ids:"), "{stderr}");
    assert!(
        stderr.contains("basic-delivery (tiers: hermetic, live)"),
        "{stderr}"
    );
}

#[test]
fn run_succeeds_for_checked_in_implementation_pr_handoff_scenario() {
    let scenario = workspace_root().join("scenarios/implementation-pr-handoff");

    let output = temper_scenario(&["run", &scenario.to_string_lossy()]);

    assert!(
        output.status.success(),
        "status: {:?}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(
        stdout.contains("scenario: implementation-pr-handoff"),
        "{stdout}"
    );
    assert!(stdout.contains("source: checked-in scenario"), "{stdout}");
    assert!(stdout.contains("confidence tier: hermetic"), "{stdout}");
    assert!(
        stdout.contains("kind: single-repo-in-memory-forge"),
        "{stdout}"
    );
    assert!(stdout.contains("verdict: passed"), "{stdout}");
    assert!(stdout.contains("create authored PR title/body"), "{stdout}");
    assert!(stdout.contains("Implement durable PR handoff"), "{stdout}");
    assert!(
        stdout.contains("refresh authored PR title/body"),
        "{stdout}"
    );
    assert!(stdout.contains("Implement refreshed handoff"), "{stdout}");
    assert!(
        stdout.contains("workflow metadata/source relation"),
        "{stdout}"
    );
    assert!(
        stdout.contains("metadata kind verified: implementation_pr"),
        "{stdout}"
    );
}

#[test]
fn validate_pr_report_records_ephemeral_source_tier_and_topology() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = dir.path().join("basic-delivery-copy");
    copy_dir_all(&workspace_root().join("scenarios/basic-delivery"), &bundle);
    let output_dir = dir.path().join("reports");

    let output = temper_scenario(&[
        "validate-pr",
        "--pr",
        "123",
        "--sha",
        "deadbeef",
        "--scenario",
        &bundle.to_string_lossy(),
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
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let report_path = PathBuf::from(stdout.trim());
    let markdown = std::fs::read_to_string(&report_path).expect("read report");
    assert!(markdown.contains("**scenario check**"), "{markdown}");
    assert!(markdown.contains("**scenario run**"), "{markdown}");
    assert!(
        markdown.contains("source: ephemeral validation bundle"),
        "{markdown}"
    );
    assert!(markdown.contains("confidence tier: hermetic"), "{markdown}");
    assert!(markdown.contains("not a live Forgejo proof"), "{markdown}");
    assert!(
        markdown.contains("manifest topology.kind: `single-repo-forgejo-standalone`"),
        "{markdown}"
    );
    assert!(
        markdown.contains("manifest topology.forge: `forgejo`"),
        "{markdown}"
    );
    assert!(
        markdown.contains("manifest topology.runner: `forgejo-actions-host`"),
        "{markdown}"
    );
    assert!(
        markdown.contains("manifest topology.temper: `standalone`"),
        "{markdown}"
    );
    assert!(
        markdown.contains("manifest topology.agent_model: `scripted-fake-llm`"),
        "{markdown}"
    );
}
