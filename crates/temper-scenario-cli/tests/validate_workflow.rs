// SPDX-License-Identifier: MPL-2.0

use std::path::PathBuf;
use std::process::{Command, Output};

fn temper_scenario(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_temper-scenario"))
        .args(args)
        .output()
        .expect("run temper-scenario")
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
        "--temper-bin",
        &missing_temper.to_string_lossy(),
        "--output-dir",
        &output_dir.to_string_lossy(),
    ]);

    assert!(
        !output.status.success(),
        "missing live temper binary should fail"
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(
        stdout.contains("run evidence:") && stdout.contains("validation result:"),
        "failure artifacts should be reported: {stdout}"
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("live manifest runner --temper-bin path does not exist"),
        "{stderr}"
    );
    assert!(stderr.contains("missing-temper"), "{stderr}");
    assert!(output_dir.is_dir(), "artifact directory should be retained");
    assert!(output_dir.join("run-evidence.json").is_file());
    let evidence: serde_json::Value = serde_json::from_slice(
        &std::fs::read(output_dir.join("run-evidence.json")).expect("read failure evidence"),
    )
    .expect("failure evidence JSON");
    assert_eq!(evidence["version"], 2);
    assert_eq!(evidence["verdict"], "failed");
    assert_eq!(evidence["scenario"]["tier"], "live");
    assert_eq!(
        evidence["scenario"]["tier_description"],
        "real Forgejo + host `forgejo-runner` CI + standalone Temper + Jig fake-LLM agents"
    );
    assert!(
        evidence["execution"]["failure"]
            .as_str()
            .is_some_and(|failure| failure.contains("missing-temper"))
    );
    assert!(
        evidence["assertions"]["blocked_required"]
            .as_u64()
            .is_some_and(|blocked| blocked > 0)
    );
    assert!(
        std::fs::read_dir(&output_dir)
            .expect("read retained artifact directory")
            .filter_map(Result::ok)
            .any(|entry| {
                entry
                    .path()
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with("validation-pr-") && name.ends_with(".json")
                    })
            }),
        "validator JSON should be retained"
    );
}

#[test]
fn validate_workflow_rejects_missing_runner_selector_before_live_setup() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scenario = dir.path().join("missing-runner");
    std::fs::create_dir_all(&scenario).expect("create scenario");
    std::fs::write(
        scenario.join("scenario.toml"),
        "schema_version = 1\n\
         name = \"missing-runner\"\n\
         status = \"ready\"\n\
         stability = \"experimental\"\n\
         intent = \"Missing runner selector should fail intentionally.\"\n",
    )
    .expect("write manifest");
    let output_dir = dir.path().join("validation-artifacts");

    let output = temper_scenario(&[
        "validate",
        "--pr",
        "123",
        "--sha",
        "deadbeef",
        "--scenario",
        &scenario.to_string_lossy(),
        "--output-dir",
        &output_dir.to_string_lossy(),
    ]);

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
    assert!(output_dir.is_dir(), "artifact directory should be retained");
}

#[test]
fn validate_workflow_rejects_legacy_tier_before_running() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scenario = workspace_root().join("scenarios/basic-delivery");
    let output_dir = dir.path().join("validation-artifacts");

    let output = temper_scenario(&[
        "validate",
        "--pr",
        "123",
        "--sha",
        "deadbeef",
        "--scenario",
        &scenario.to_string_lossy(),
        "--output-dir",
        &output_dir.to_string_lossy(),
        "--tier=live",
    ]);

    assert_eq!(output.status.code(), Some(64));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("unexpected argument `--tier=live`"),
        "{stderr}"
    );
    assert!(!output_dir.exists(), "scenario execution must not begin");
}
