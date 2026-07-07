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

fn set_runner_for_bundle(bundle: &Path, runner: Option<&str>) {
    let manifest_path = bundle.join("scenario.toml");
    let manifest = std::fs::read_to_string(&manifest_path).expect("read manifest");
    let rewritten = if let Some(runner) = runner {
        manifest.replace("uses = \"manifest\"", &format!("uses = \"{runner}\""))
    } else {
        manifest
            .replace("[runner]\nuses = \"manifest\"\n\n", "")
            .replace("[runner]\nuses = \"manifest\"\r\n\r\n", "")
    };
    std::fs::write(&manifest_path, rewritten).expect("write manifest");
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
fn checked_in_manifest_scenarios_reject_explicit_hermetic_tier() {
    for scenario_name in [
        "basic-delivery",
        "implementation-pr-handoff",
        "codebase-memory-agent",
    ] {
        let scenario = workspace_root().join("scenarios").join(scenario_name);

        let output = temper_scenario(&["run", "--tier", "hermetic", &scenario.to_string_lossy()]);

        assert!(
            !output.status.success(),
            "manifest runner must reject hermetic tier for {scenario_name}"
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout), "");
        let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
        assert!(
            stderr.contains("unsupported tier `hermetic` for runner `manifest`"),
            "{stderr}"
        );
        assert!(stderr.contains("runner.uses"), "{stderr}");
        assert!(stderr.contains("supported tiers: live"), "{stderr}");
        assert!(
            stderr.contains("no hermetic, MemoryForge, or in-process substitute"),
            "{stderr}"
        );
        assert!(!stderr.contains("basic-delivery (tiers"), "{stderr}");
    }
}

#[test]
fn run_live_tier_with_missing_temper_binary_selects_manifest_only() {
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
        stderr.contains("live manifest runner --temper-bin path does not exist"),
        "{stderr}"
    );
    assert!(stderr.contains("missing-temper"), "{stderr}");
    assert!(!stderr.contains("seeded issue:"), "{stderr}");
    assert!(!stderr.contains("basic-delivery (tiers"), "{stderr}");
}

#[test]
fn run_rejects_missing_runner_selector_instead_of_falling_back_to_name() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = dir.path().join("basic-delivery-copy");
    copy_dir_all(&workspace_root().join("scenarios/basic-delivery"), &bundle);
    set_runner_for_bundle(&bundle, None);

    let output = temper_scenario(&["run", &bundle.to_string_lossy()]);

    assert!(
        !output.status.success(),
        "missing runner selector should fail"
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("does not declare `[runner] uses = \"manifest\"`"),
        "{stderr}"
    );
    assert!(
        stderr.contains("legacy scenario-name fallback has been removed"),
        "{stderr}"
    );
    assert!(stderr.contains("will not dispatch by `name`"), "{stderr}");
}

#[test]
fn run_rejects_retired_basic_delivery_runner_alias() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = dir.path().join("basic-delivery-copy");
    copy_dir_all(&workspace_root().join("scenarios/basic-delivery"), &bundle);
    set_runner_for_bundle(&bundle, Some("basic-delivery"));

    let output = temper_scenario(&["run", "--tier", "live", &bundle.to_string_lossy()]);

    assert!(!output.status.success(), "retired alias should fail");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("unsupported runner `basic-delivery` selected by runner.uses"),
        "{stderr}"
    );
    assert!(
        stderr.contains("no compatibility aliases are registered"),
        "{stderr}"
    );
    assert!(stderr.contains("manifest (tiers: live)"), "{stderr}");
    assert!(!stderr.contains("basic-delivery (tiers"), "{stderr}");
}

#[test]
fn run_help_documents_manifest_only_stack() {
    let output = temper_scenario(&["run", "--help"]);

    assert!(output.status.success(), "status: {:?}", output.status);
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(
        stdout.contains("Usage: temper-scenario run [--tier <live|hermetic>] [--temper-bin <PATH>] [--evidence-out <PATH>] <SCENARIO_PATH>"),
        "{stdout}"
    );
    assert!(stdout.contains("default: live"), "{stdout}");
    assert!(
        stdout.contains("The only registered scenario runner is `manifest`"),
        "{stdout}"
    );
    assert!(stdout.contains("real forgejo-runner CI"), "{stdout}");
    assert!(
        stdout.contains("legacy manifest `name` fallback has been removed"),
        "{stdout}"
    );
    assert!(!stdout.contains("basic-delivery`"), "{stdout}");
}

#[test]
fn run_rejects_unknown_tier() {
    let scenario = workspace_root().join("scenarios/basic-delivery");

    let output = temper_scenario(&["run", "--tier", "medium", &scenario.to_string_lossy()]);

    assert!(!output.status.success(), "unknown tier should fail");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("unknown --tier `medium`"), "{stderr}");
    assert!(stderr.contains("expected live or hermetic"), "{stderr}");
}

#[test]
fn validate_pr_live_missing_temper_binary_writes_failed_report() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scenario = workspace_root().join("scenarios/implementation-pr-handoff");
    let missing_temper = dir.path().join("missing-temper");
    let output_dir = dir.path().join("reports");

    let output = temper_scenario(&[
        "validate-pr",
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
        "missing live temper binary should fail report"
    );
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let markdown = std::fs::read_to_string(PathBuf::from(stdout.trim())).expect("read report");
    assert!(markdown.contains("Verdict: failed"), "{markdown}");
    assert!(markdown.contains("Live scenario run failed"), "{markdown}");
    assert!(
        markdown.contains("live manifest runner --temper-bin path does not exist"),
        "{markdown}"
    );
    assert!(
        !markdown.contains("implementation-pr-handoff (tiers: hermetic)"),
        "{markdown}"
    );
}
