// SPDX-License-Identifier: MPL-2.0

use serde_json::Value;

use crate::support::{temper, write_valid_bundle};

#[test]
fn check_fails_without_config_files_but_prints_report() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = temper(&["check"], dir.path());

    assert!(
        !output.status.success(),
        "missing config should report blocking validation findings"
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
        !output.status.success(),
        "JSON status=error should also return a non-zero process status"
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
fn check_json_succeeds_for_valid_explicit_bundle() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = write_valid_bundle(dir.path());
    let bundle_arg = bundle.to_string_lossy();
    let output = temper(
        &["--config", &bundle_arg, "--format", "json", "check"],
        dir.path(),
    );

    assert!(
        output.status.success(),
        "status: {:?}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let value: Value = serde_json::from_str(&stdout).expect("valid JSON");

    assert_eq!(value["status"], "ok");
    assert_eq!(value["result"], "ok");
    assert_eq!(
        value["paths"]["config"],
        bundle.join("config.toml").display().to_string()
    );
    assert_eq!(
        value["paths"]["credentials"],
        bundle.join("credentials.toml").display().to_string()
    );
    assert!(
        value["findings"].as_array().is_some_and(Vec::is_empty),
        "{value}"
    );
}
