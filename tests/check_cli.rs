// SPDX-License-Identifier: MPL-2.0

use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

fn temper(args: &[&str], env_root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_temper"))
        .args(args)
        .env("XDG_CONFIG_HOME", env_root.join("xdg-config"))
        .env("XDG_STATE_HOME", env_root.join("xdg-state"))
        .env("HOME", env_root.join("home"))
        .output()
        .expect("run temper")
}

#[test]
fn top_level_help_lists_check_and_hides_internal_agent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = temper(&["--help"], dir.path());

    assert!(output.status.success(), "status: {:?}", output.status);
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("\n  check "), "{stdout}");
    assert!(!stdout.contains("\n  agent "), "{stdout}");
}

#[test]
fn check_help_exits_successfully() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = temper(&["check", "--help"], dir.path());

    assert!(output.status.success(), "status: {:?}", output.status);
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(
        stdout.contains("Usage: temper [GLOBAL OPTIONS] check"),
        "{stdout}"
    );
}

#[test]
fn check_succeeds_without_config_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = temper(&["check"], dir.path());

    assert!(
        output.status.success(),
        "status: {:?}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(
        stdout.contains("config:      (none"),
        "default environment should not load a config file: {stdout}"
    );
    assert!(
        stdout.contains("credentials: (none"),
        "default environment should not load credentials: {stdout}"
    );
    assert!(stdout.contains("error: forge URL is unset"), "{stdout}");
}

#[test]
fn check_json_reports_status_findings_and_loaded_paths() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = temper(&["--format", "json", "check"], dir.path());

    assert!(
        output.status.success(),
        "status: {:?}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let value: Value = serde_json::from_str(&stdout).expect("valid JSON");

    assert_eq!(value["status"], "error");
    assert_eq!(value["result"], "error");
    assert!(value["config_path"].is_null(), "{value}");
    assert!(value["credentials_path"].is_null(), "{value}");
    assert!(value["paths"]["config"].is_null(), "{value}");
    assert!(value["paths"]["credentials"].is_null(), "{value}");

    let findings = value["findings"].as_array().expect("findings array");
    assert!(
        findings.iter().any(|finding| finding["severity"] == "error"
            && finding["message"]
                .as_str()
                .is_some_and(|message| message.contains("forge URL is unset"))),
        "{value}"
    );
    assert!(
        findings.iter().any(|finding| finding["severity"] == "note"),
        "{value}"
    );
}

#[test]
fn config_validate_remains_dispatchable_for_compatibility() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = temper(&["config", "validate"], dir.path());

    assert!(
        !output.status.success(),
        "compatibility path should preserve strict validation status"
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("config:      (none"), "{stdout}");
    assert!(stdout.contains("error: forge URL is unset"), "{stdout}");
}
